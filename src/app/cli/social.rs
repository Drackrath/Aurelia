//! Friends, market, chat and wishlist subcommands.

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum FriendsCommand {
    /// List your friends (the default when no subcommand is given).
    List,
    /// Show a user by id, URL, name.
    Search { query: String },
    /// Send a friend request (same identifiers as search).
    Add { query: String },
    /// Remove a friend, or cancel/decline a pending request (by SteamID64).
    Remove { steamid: u64 },
}

#[derive(Subcommand)]
pub(crate) enum MarketCommand {
    /// Item price (community web; no CM equivalent).
    Price {
        app_id: u32,
        /// Exact market hash name (case-sensitive), e.g. "Mann Co. Supply Crate Key".
        name: String,
        /// Steam currency id (1=USD, 2=GBP, 3=EUR, …).
        #[arg(short = 'c', long, default_value_t = 1)]
        currency: u32,
    },
    /// Search the market (community web; no CM equivalent).
    Search {
        /// Free-text query (optional).
        query: Option<String>,
        /// Restrict to one game by app id.
        #[arg(short = 'a', long)]
        app_id: Option<u32>,
        /// Maximum results to return.
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
    },
    /// Show your active market listings and open buy orders.
    Listings,
}

#[derive(Subcommand)]
pub(crate) enum ChatCommand {
    /// Send a direct message to a friend (by SteamID64).
    Send {
        steamid: u64,
        /// The message text (all remaining words are joined with spaces).
        #[arg(required = true, trailing_var_arg = true)]
        message: Vec<String>,
    },
    /// Invite a friend (by SteamID64) into your running game.
    Invite {
        steamid: u64,
        /// App id to invite to.
        #[arg(short = 'a', long = "app")]
        app_id: u32,
        /// Optional game connect string.
        #[arg(short = 'c', long)]
        connect: Option<String>,
    },
    /// Show recent messages exchanged with a friend (by SteamID64).
    History {
        steamid: u64,
        /// How many recent messages to fetch.
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
    },
    /// Open an interactive live chat with a friend: type lines to send, incoming
    /// messages stream in. End with Ctrl-D (Ctrl-Z on Windows). With `--json`,
    /// emits one JSON event per line and reads message text from stdin lines.
    Open { steamid: u64 },
}

/// `wishlist add|remove`.
#[derive(Subcommand)]
pub(crate) enum WishlistCommand {
    /// Add an app to your wishlist.
    Add { app_id: u32 },
    /// Remove an app from your wishlist.
    Remove { app_id: u32 },
}
