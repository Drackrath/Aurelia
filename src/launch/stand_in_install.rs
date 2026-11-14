//! Which Steam install stand-in launches use.

use std::path::{Path, PathBuf};

use crate::core::models::StandInSteamInstall;

/// Resolved install for one launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    /// Host Steam root; host compatdata prefix.
    Host(PathBuf),
    /// Master prefix (`.../pfx`).
    InWine(PathBuf),
}

/// Steam folder inside a prefix.
const PREFIX_STEAM_DIR: &str = "drive_c/Program Files (x86)/Steam";

/// Proton-usable master with in-Wine Steam.
pub fn in_wine_ready(master_prefix: &Path) -> bool {
    // Proton needs `<compat>/pfx`.
    master_prefix.file_name().is_some_and(|n| n == "pfx")
        && master_prefix.join(PREFIX_STEAM_DIR).is_dir()
}

/// Per-game setting wins; auto prefers in-Wine.
pub fn choose(
    global: StandInSteamInstall,
    per_game: Option<StandInSteamInstall>,
    host_root: Option<PathBuf>,
    master_prefix: &Path,
) -> Result<Install, String> {
    let in_wine =
        in_wine_ready(master_prefix).then(|| Install::InWine(master_prefix.to_path_buf()));
    let host = host_root.map(Install::Host);
    match per_game.unwrap_or(global) {
        StandInSteamInstall::InWine => in_wine.ok_or_else(|| {
            format!(
                "--steam-install in-wine: no in-Wine Steam in {}; run `aurelia steam-runtime install` \
                 or use --steam-install host",
                master_prefix.display()
            )
        }),
        StandInSteamInstall::Host => host.ok_or_else(|| {
            "--steam-install host: no host Steam install found; install Valve's Steam client \
             or use --steam-install in-wine"
                .to_string()
        }),
        StandInSteamInstall::Auto => in_wine.or(host).ok_or_else(|| {
            "the stand-in needs a Steam install for Proton's steamclient files: run \
             `aurelia steam-runtime install` (in-Wine Steam) or install Valve's Steam client"
                .to_string()
        }),
    }
}

/// Prefix layout for one launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// `STEAM_COMPAT_DATA_PATH`.
    pub compat_data: PathBuf,
    /// `WINEPREFIX`, `compat_data/pfx`.
    pub prefix: PathBuf,
    /// `STEAM_COMPAT_CLIENT_INSTALL_PATH`; `None` keeps the trap.
    pub client_install: Option<PathBuf>,
}

/// Paths Proton must agree on.
pub fn layout(install: &Install, host_compat_data: &Path) -> Layout {
    match install {
        // Proton copies legacycompat there.
        Install::Host(root) => Layout {
            compat_data: host_compat_data.to_path_buf(),
            prefix: host_compat_data.join("pfx"),
            client_install: Some(root.clone()),
        },
        // Binds shadow the Wine DLLs.
        Install::InWine(master) => {
            let compat = match master.parent() {
                Some(parent) if master.file_name().is_some_and(|n| n == "pfx") => {
                    parent.to_path_buf()
                }
                _ => master.clone(),
            };
            Layout {
                prefix: compat.join("pfx"),
                compat_data: compat,
                client_install: None,
            }
        }
    }
}

/// Launch log line.
pub fn describe(install: &Install, layout: &Layout) -> String {
    let kind = match install {
        Install::Host(_) => "host",
        Install::InWine(_) => "in-wine",
    };
    format!(
        "stand-in Steam install: {kind}; prefix {}; STEAM_COMPAT_CLIENT_INSTALL_PATH {}",
        layout.prefix.display(),
        layout
            .client_install
            .as_ref()
            .map_or("fake trap".to_string(), |p| p.display().to_string())
    )
}

#[cfg(test)]
#[path = "stand_in_install_tests.rs"]
mod tests;
