//! `cloud` subcommands.

use std::path::PathBuf;

use clap::Subcommand;

use super::*;

#[derive(Subcommand)]
pub(crate) enum CloudCommand {
    /// Sync a game's Steam Cloud saves with the local save directory. With
    /// neither flag it syncs down then up; `--down`/`--up` restrict the direction.
    Sync {
        app_id: u32,
        /// Only upload local saves to Steam.
        #[arg(short = 'u', long, conflicts_with = "down")]
        up: bool,
        /// Only download saves from Steam.
        #[arg(short = 'd', long, conflicts_with = "up")]
        down: bool,
        /// Local save directory. Defaults to Aurelia's managed cloud root.
        #[arg(short = 'p', long)]
        path: Option<PathBuf>,
        /// Resolve diverged saves by taking this side (`cloud` or `local`) instead
        /// of reporting them. Omit to only detect conflicts and leave both copies.
        #[arg(short = 'r', long, value_enum)]
        resolve: Option<CloudResolve>,
    },
    /// List a game's Steam Cloud files (name, size, modified time).
    List { app_id: u32 },
}
