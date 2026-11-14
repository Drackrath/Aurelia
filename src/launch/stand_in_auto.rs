//! `steamworks-client auto` per-launch choice.

use crate::core::config::LauncherConfig;

/// How one launch treats the stand-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Switch is off.
    Off,
    /// Switch is on.
    On,
    /// Auto: Steam down, library installed.
    StandIn,
    /// Auto: Valve's Steam is running.
    SteamRunning,
    /// Auto: nothing installed.
    NotInstalled,
    /// Auto: library fails its hash check.
    Unverified,
}

/// Installed stand-in library state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Library {
    /// Installed with a matching hash.
    Ready,
    Missing,
    /// Hash missing or different.
    Unverified,
}

impl Choice {
    pub fn uses_stand_in(self) -> bool {
        matches!(self, Self::On | Self::StandIn)
    }

    /// One line for auto launches.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Self::StandIn => Some("stand-in: Valve's Steam client is not running"),
            Self::SteamRunning => {
                Some("Valve Steam: the Steam client is running, stand-in not used")
            }
            Self::NotInstalled => Some(
                "Valve Steam: no stand-in library installed (aurelia config \
                 steamworks-client --path ...)",
            ),
            Self::Unverified => Some(
                "Valve Steam: the stand-in library has no recorded hash or changed since \
                 it was installed, so this launch uses the normal way; reinstall it with \
                 `aurelia config steamworks-client --path ...`",
            ),
            Self::Off | Self::On => None,
        }
    }
}

/// Decide; detectors run lazily.
pub fn decide(
    config: &LauncherConfig,
    steam_running: impl FnOnce() -> bool,
    installed: impl FnOnce() -> Library,
) -> Choice {
    if config.steamworks_client {
        return Choice::On;
    }
    if !config.steamworks_client_auto {
        return Choice::Off;
    }
    if steam_running() {
        return Choice::SteamRunning;
    }
    match installed() {
        Library::Ready => Choice::StandIn,
        Library::Missing => Choice::NotInstalled,
        Library::Unverified => Choice::Unverified,
    }
}

/// Sets this launch's stand-in flag.
pub fn apply(
    config: &mut LauncherConfig,
    steam_running: impl FnOnce() -> bool,
    installed: impl FnOnce(&LauncherConfig) -> Library,
) -> Choice {
    let snapshot = config.clone();
    let choice = decide(&snapshot, steam_running, || installed(&snapshot));
    // In memory only, never saved.
    config.steamworks_client = choice.uses_stand_in();
    choice
}

/// Stand-in libraries present and verified.
#[cfg(target_os = "linux")]
pub fn installed(config: &LauncherConfig) -> Library {
    use crate::launch::steamworks_overlay::{default_client_dir, resolve_client_libs};
    let libs = config
        .steamworks_client_path
        .as_deref()
        .map(std::path::PathBuf::from)
        .or_else(default_client_dir)
        .and_then(|dir| resolve_client_libs(&dir).ok());
    let Some(libs) = libs else {
        return Library::Missing;
    };
    // Same check the launch makes.
    for lib in std::iter::once(&libs.sdk64).chain(libs.sdk32.as_ref()) {
        if let Err(e) = crate::launch::stand_in_lib::verify(lib) {
            tracing::warn!("{e:#}");
            return Library::Unverified;
        }
    }
    Library::Ready
}

#[cfg(not(target_os = "linux"))]
pub fn installed(_config: &LauncherConfig) -> Library {
    Library::Missing
}

/// Note for refused games in auto.
pub fn refused_fallback_note(app_id: u32, name: &str, labels: &str) -> String {
    format!(
        "Valve Steam: {name} ({app_id}) uses {labels}, which the stand-in must not run; \
         auto mode launches it the normal way instead"
    )
}

#[cfg(test)]
#[path = "stand_in_auto_tests.rs"]
mod tests;
