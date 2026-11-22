//! `depots` arguments and subcommands.

use std::path::PathBuf;

/// `aurelia depots`: list, or browse files.
#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub(crate) struct DepotsArgs {
    #[command(subcommand)]
    pub(crate) command: Option<DepotsCommand>,
    #[arg(required = true)]
    pub(crate) app_id: Option<u32>,
}

#[derive(clap::Subcommand)]
pub(crate) enum DepotsCommand {
    /// List a depot manifest's files.
    Files {
        app_id: u32,
        depot_id: u32,
        #[command(flatten)]
        manifest: DepotManifestArgs,
        /// Glob filter (`*`, `?`, case-insensitive).
        #[arg(short, long)]
        filter: Option<String>,
    },
    /// Download one depot manifest file.
    Download {
        app_id: u32,
        depot_id: u32,
        /// File path inside the depot.
        path: String,
        #[command(flatten)]
        manifest: DepotManifestArgs,
        /// Save directory (default: current directory).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Overwrite an existing file.
        #[arg(short, long)]
        force: bool,
    },
}

/// Manifest id, or branch's current one.
#[derive(clap::Args)]
pub(crate) struct DepotManifestArgs {
    /// Manifest id (default: branch's current).
    #[arg(short, long, conflicts_with = "branch")]
    pub(crate) manifest: Option<u64>,
    /// Branch to read (default: public).
    #[arg(short, long)]
    pub(crate) branch: Option<String>,
}
