//! Collections, reviews and deals args.

use clap::{Subcommand, ValueEnum};

#[derive(Subcommand)]
pub(crate) enum CollectionsCommand {
    /// List all collections and their game counts (offline).
    List,
    /// Show a collection's games (offline). Accepts a name or id.
    Show { name: String },
    /// Create a new (static) collection (offline).
    Create { name: String },
    /// Delete a collection (offline). Built-in favorite/hidden can't be deleted.
    Delete { name: String },
    /// Rename a collection (offline).
    Rename { name: String, new_name: String },
    /// Add one or more app ids to a collection (offline).
    Add {
        name: String,
        #[arg(required = true)]
        app_ids: Vec<u32>,
    },
    /// Remove one or more app ids from a collection (offline).
    Remove {
        name: String,
        #[arg(required = true)]
        app_ids: Vec<u32>,
    },
    /// Download collections from your Steam account and merge them in (needs login).
    Pull,
    /// Upload your local collections to your Steam account (needs login).
    /// This changes your real Steam library — confirm or pass `--yes`.
    Push {
        /// Skip the confirmation prompt. Required in `--json` mode.
        #[arg(short, long)]
        yes: bool,
        /// Push even if never pulled.
        #[arg(long)]
        force: bool,
    },
    /// Pull then push — reconcile local and Steam collections (needs login).
    /// This changes your real Steam library — confirm or pass `--yes`.
    Sync {
        /// Skip the confirmation prompt. Required in `--json` mode.
        #[arg(short, long)]
        yes: bool,
    },
}

/// `aurelia reviews` flags.
#[derive(clap::Args)]
pub(crate) struct ReviewsArgs {
    pub(crate) app_id: u32,
    /// `recent`, `updated` or `all` (score-weighted).
    #[arg(short = 'f', long, default_value = "recent")]
    pub(crate) filter: String,
    /// `all`, `positive` or `negative`.
    #[arg(short = 't', long = "type", default_value = "all")]
    pub(crate) review_type: String,
    /// `all`, `steam` or `non_steam_purchase`.
    #[arg(short = 'p', long, default_value = "all")]
    pub(crate) purchase: String,
    /// Maximum reviews per page (up to 100).
    #[arg(short = 'n', long, default_value_t = 10)]
    pub(crate) count: u32,
    /// Cursor from a previous page's JSON.
    #[arg(short = 'u', long)]
    pub(crate) cursor: Option<String>,
    /// Steam API language name, or `all`.
    #[arg(short = 'l', long = "lang", alias = "language")]
    pub(crate) lang: Option<String>,
}

/// `deals --scope` choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum DealsScopeArg {
    /// Discounted items among regional top sellers.
    DiscountedTopSellers,
    /// Regional top sellers, discounted or not.
    TopSellers,
    /// Anything discounted at least `--min-discount`.
    Specials,
}
