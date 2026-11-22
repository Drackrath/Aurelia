//! `install` args and actions.

use super::*;

/// `aurelia install`: either install an app (positional `app_id`) or manage
/// in-flight installs (`list` / `stop`). The positional and the subcommands
/// coexist via clap's args-conflict-with-subcommands pattern.
#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub(crate) struct InstallArgs {
    #[command(subcommand)]
    pub(crate) action: Option<InstallAction>,
    /// App id to install. Auto-detected platform unless --platform given.
    pub(crate) app_id: Option<u32>,
    /// Depot platform to install. Auto-detected if omitted.
    #[arg(short, long)]
    pub(crate) platform: Option<PlatformArg>,
    /// When installing a DLC, restart the Steam client afterward so the running
    /// client picks up the change (Windows and Linux). Without this it only warns.
    #[arg(short = 'r', long)]
    pub(crate) restart_steam: bool,
    /// Don't install — just report the estimated download and on-disk size
    /// and the language choices (from PICS, no files fetched). Pair with
    /// `--json` for tooling.
    #[arg(short = 'd', long)]
    pub(crate) dry_run: bool,
    /// Steam library folder (drive/location) to install into. A library root
    /// containing a `steamapps` directory, as listed by `aurelia libraries`.
    /// Defaults to the configured `steam_library_path`.
    #[arg(short = 'l', long)]
    pub(crate) library: Option<String>,
    /// Steam language to install, e.g. `german`. Must be one of the
    /// `languages` that `--dry-run --json` lists; recorded like a Steam
    /// language choice. Defaults to the manifest, Steam client or config
    /// language, falling back to English.
    #[arg(long, value_name = "LANGUAGE")]
    pub(crate) language: Option<String>,
}

#[derive(clap::Subcommand)]
pub(crate) enum InstallAction {
    /// List running installs and the backlog (paused, aborted, interrupted).
    List,
    /// Stop a running install by app id. It stays in the backlog as `aborted`.
    Stop { app_id: u32 },
    /// Pause a running install, keeping what's on disk. Resume it with
    /// `install resume`; automatic resume never picks it up.
    Pause { app_id: u32 },
    /// Resume a backlog install with the options it was started with, reusing
    /// the files already on disk.
    Resume {
        /// App id to resume.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        app_id: Option<u32>,
        /// Resume every backlog entry, one after another.
        #[arg(short, long)]
        all: bool,
    },
    /// Remove an install from the backlog.
    Forget {
        app_id: u32,
        /// Also delete its partial files (never a DLC's or a finished game's folder).
        #[arg(short, long)]
        delete_files: bool,
    },
}

/// `aurelia downgrade`: install specific depot manifests (an older version) and
/// pin them. `--depot`/`--manifest` are parallel repeatable lists paired by
/// position; `--manifest <depot>:<manifest>` is an alternative combined form.
#[derive(clap::Args)]
pub(crate) struct DowngradeArgs {
    /// App id to downgrade.
    pub(crate) app_id: u32,
    /// Target depot id (repeatable). Paired by position with a bare `--manifest`.
    #[arg(short = 'd', long = "depot", value_name = "DEPOT_ID")]
    pub(crate) depots: Vec<u32>,
    /// Target manifest id (repeatable). Either a bare id (paired by position with
    /// `--depot`) or the combined `<depot>:<manifest>` form.
    #[arg(short = 'm', long = "manifest", value_name = "MANIFEST_ID")]
    pub(crate) manifests: Vec<String>,
    /// Branch whose build id to record in the appmanifest (default: public).
    #[arg(short = 'b', long)]
    pub(crate) branch: Option<String>,
    /// Password for a protected branch (recorded only; see the downgrade docs).
    #[arg(short = 'w', long)]
    pub(crate) branch_password: Option<String>,
    /// Steam library folder (drive/location) to install into.
    #[arg(short = 'l', long)]
    pub(crate) library: Option<String>,
    /// Verify the install after downloading (integrity pass).
    #[arg(short = 'V', long)]
    pub(crate) verify: bool,
    /// Don't pin after downgrading (Aurelia's update commands may re-upgrade it).
    #[arg(short = 'N', long)]
    pub(crate) no_pin: bool,
}
