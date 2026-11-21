//! `workshop` subcommands.

use clap::Subcommand;

use super::*;

#[derive(Subcommand)]
pub(crate) enum WorkshopCommand {
    /// Browse/search a game's Workshop to discover items to subscribe to or install.
    Browse {
        app_id: u32,
        /// Filter by search text (free-text match on title/description).
        #[arg(short, long)]
        search: Option<String>,
        /// Sort order.
        #[arg(short = 'o', long, value_enum, default_value_t = WorkshopSort::Trend)]
        sort: WorkshopSort,
        /// Number of results per page (1–100).
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
        /// Pagination cursor; use a previous page's `next_cursor` (`*` = first page).
        #[arg(short = 'c', long, default_value = "*")]
        cursor: String,
        /// Only items carrying this tag (repeatable).
        #[arg(short = 't', long = "tag")]
        tags: Vec<String>,
    },
    /// Show metadata for one or more Workshop items (or collections).
    Info {
        /// One or more Workshop published-file ids.
        #[arg(required = true)]
        ids: Vec<u64>,
    },
    /// List the Workshop items you're subscribed to for a game.
    List { app_id: u32 },
    /// Download one or more Workshop items (or collections) and register them.
    Install {
        #[arg(required = true)]
        ids: Vec<u64>,
        /// Install only the given ids; do not expand collections to their members.
        #[arg(short = 'R', long)]
        no_recurse: bool,
    },
    /// Remove one or more installed Workshop items (or collections).
    Uninstall {
        #[arg(required = true)]
        ids: Vec<u64>,
        /// Uninstall only the given ids; do not expand collections to their members.
        #[arg(short = 'R', long)]
        no_recurse: bool,
    },
    /// Subscribe to one or more Workshop items (or collections).
    Subscribe {
        #[arg(required = true)]
        ids: Vec<u64>,
        /// Also download the content after subscribing.
        #[arg(short = 'i', long)]
        install: bool,
        /// Subscribe only to the given ids; do not expand collections to their members.
        #[arg(short = 'R', long)]
        no_recurse: bool,
    },
    /// Unsubscribe from one or more Workshop items (or collections).
    Unsubscribe {
        #[arg(required = true)]
        ids: Vec<u64>,
        /// Unsubscribe only from the given ids; do not expand collections.
        #[arg(short = 'R', long)]
        no_recurse: bool,
    },
    /// Show installed vs subscribed Workshop items for a game.
    Status { app_id: u32 },
    /// Rate a Workshop item thumbs-up or thumbs-down.
    Rate {
        /// The Workshop published-file id.
        id: u64,
        /// `up` or `down`.
        vote: VoteArg,
    },
    /// Read the comments on a Workshop item.
    Comments {
        /// The Workshop published-file id.
        id: u64,
        /// How many comments to fetch (1–100).
        #[arg(short = 'n', long, default_value_t = 20)]
        count: i32,
        /// Index of the first comment to fetch (for paging).
        #[arg(short = 's', long, default_value_t = 0)]
        start: i32,
    },
    /// Post a comment to a Workshop item.
    Comment {
        /// The Workshop published-file id.
        id: u64,
        /// The comment text.
        text: String,
    },
}
