//! Compatibility tool subcommands.

use clap::Subcommand;

use super::*;

#[derive(Subcommand)]
pub(crate) enum ProtonCommand {
    /// List installable runtimes (Valve + GE) and what's already installed.
    List {
        /// Only show what's installed on disk (skips the GitHub/Valve lookup).
        #[arg(short = 'i', long)]
        installed: bool,
    },
    /// Download and install a runtime by name (from `proton list`).
    Install {
        /// Runtime name, e.g. `GE-Proton9-20` or `Proton 9.0`.
        version: String,
    },
    /// Uninstall an installed custom (GE) runtime from compatibilitytools.d.
    Uninstall { version: String },
    /// Set the global default Proton/Wine version (used when a game has none set).
    Default { version: String },
    /// Find Steam mappings to missing tools.
    Mappings,
    /// Install a Proton anti-cheat runtime.
    Anticheat {
        /// Which runtime to install.
        runtime: AntiCheatArg,
    },
}

#[derive(Subcommand)]
pub(crate) enum SteamRuntimeCommand {
    /// Download SteamSetup.exe (if needed) and install Steam into the master prefix.
    /// Requires `steam_runtime_runner` to be configured.
    Install {
        /// Remove the existing master Steam prefix first, then install a fresh copy.
        /// Use when Steam reports a corrupted install ("please reinstall"). Unlike
        /// `repair` (which keeps a `.bak`), this deletes the old prefix outright.
        #[arg(short = 'r', long)]
        reinstall: bool,
    },
    /// Stop Steam, back up the master prefix (timestamped `.bak-*`), then reinstall.
    /// Requires `steam_runtime_runner` to be configured.
    Repair,
    /// Remove the master Windows Steam prefix entirely (uninstall the runtime).
    /// Stops any Steam running in it first, then deletes the whole master prefix
    /// (including any `.bak-*`). Reinstall later with `steam-runtime install`.
    Uninstall,
    /// (Re-)start the in-Wine Steam client interactively so you can sign in — use
    /// after the runtime's Steam session expired, or to switch accounts. Does NOT
    /// reinstall. The in-Wine Steam keeps its own login (separate from `aurelia
    /// login`). Requires `steam_runtime_runner` and an installed runtime.
    #[command(visible_alias = "relogin")]
    Login,
    /// Stop the in-Wine Steam client running in the master prefix (kills its Wine
    /// session), without uninstalling. Use to shut down a Steam started by `login`
    /// or one left running after a game launch.
    Stop,
    /// Show the resolved master prefix, layout, whether steam.exe is present, and
    /// whether a Steam-runtime runner is configured.
    Status,
}

#[derive(Subcommand)]
pub(crate) enum LuxtorpedaCommand {
    /// Enable the plugin (sets the master toggle; games still opt in per-game).
    Enable,
    /// Disable the plugin. Games pinned to it fall back to normal native/Proton launch.
    Disable,
    /// Download (or re-download) the latest luxtorpeda client into Aurelia's data dir.
    Install,
    /// Re-fetch the latest luxtorpeda client, replacing the installed payload.
    Update,
    /// Show whether the plugin is enabled and which version (if any) is installed.
    Status,
    /// Use an externally-managed luxtorpeda install instead of the managed download.
    /// Pass a directory to set it (disables downloading), omit args to show the current
    /// value, or `--clear` to revert to the managed download.
    Path {
        /// Directory of an existing luxtorpeda install (contains `toolmanifest.vdf`).
        path: Option<String>,
        /// Clear the custom path and use Aurelia's managed download instead.
        #[arg(short = 'c', long, conflicts_with = "path")]
        clear: bool,
    },
    /// Remove the downloaded luxtorpeda payload from disk.
    Uninstall,
}

#[derive(Subcommand)]
pub(crate) enum UmuCommand {
    /// Enable the plugin (sets the master toggle; games still opt in per-game).
    Enable,
    /// Disable the plugin. Games pinned to it fall back to normal native/Proton launch.
    Disable,
    /// Download (or re-download) the latest umu-launcher into Aurelia's data dir.
    Install,
    /// Re-fetch the latest umu-launcher, replacing the installed payload.
    Update,
    /// Show whether the plugin is enabled and which version (if any) is installed.
    Status,
    /// Use an externally-managed umu install instead of the managed download.
    /// Pass a directory (or the `umu-run` binary) to set it (disables downloading),
    /// omit args to show the current value, or `--clear` to revert to the managed download.
    Path {
        /// Directory of an existing umu install (contains `umu-run`), or the `umu-run` binary.
        path: Option<String>,
        /// Clear the custom path and use Aurelia's managed download instead.
        #[arg(short = 'c', long, conflicts_with = "path")]
        clear: bool,
    },
    /// Remove the downloaded umu-launcher payload from disk.
    Uninstall,
}

#[derive(Subcommand)]
pub(crate) enum ScriptsCommand {
    /// Print the resolved launch-script directory (`AURELIA_SCRIPT_DIR` or
    /// `<config_dir>/scripts`).
    Dir,
    /// List app ids that have a launch script (dir-based and config-pinned) and
    /// their resolved paths.
    List,
    /// Scaffold a launch script for a game at `<script_dir>/<app_id>.sh` (or `.bat`
    /// on Windows). Errors if one already exists unless `--force`.
    New {
        app_id: u32,
        /// Overwrite an existing script.
        #[arg(short = 'f', long)]
        force: bool,
    },
    /// Print the resolved launch-script path for a game and its contents.
    Show { app_id: u32 },
    /// Delete the dir-based launch script for a game.
    Remove { app_id: u32 },
}
