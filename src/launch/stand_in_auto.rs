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
            Self::Off | Self::On => None,
        }
    }
}

/// Decide; detectors run lazily.
pub fn decide(
    config: &LauncherConfig,
    steam_running: impl FnOnce() -> bool,
    installed: impl FnOnce() -> bool,
) -> Choice {
    if config.steamworks_client {
        return Choice::On;
    }
    if !config.steamworks_client_auto {
        return Choice::Off;
    }
    if steam_running() {
        Choice::SteamRunning
    } else if installed() {
        Choice::StandIn
    } else {
        Choice::NotInstalled
    }
}

/// Sets this launch's stand-in flag.
pub fn apply(
    config: &mut LauncherConfig,
    steam_running: impl FnOnce() -> bool,
    installed: impl FnOnce(&LauncherConfig) -> bool,
) -> Choice {
    let snapshot = config.clone();
    let choice = decide(&snapshot, steam_running, || installed(&snapshot));
    // In memory only, never saved.
    config.steamworks_client = choice.uses_stand_in();
    choice
}

/// A 64-bit stand-in is installed.
#[cfg(target_os = "linux")]
pub fn installed(config: &LauncherConfig) -> bool {
    use crate::launch::steamworks_overlay::{default_client_dir, resolve_client_libs};
    config
        .steamworks_client_path
        .as_deref()
        .map(std::path::PathBuf::from)
        .or_else(default_client_dir)
        .is_some_and(|dir| resolve_client_libs(&dir).is_ok())
}

#[cfg(not(target_os = "linux"))]
pub fn installed(_config: &LauncherConfig) -> bool {
    false
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
