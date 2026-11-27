//! clap CLI type definitions (parsed command surface).

use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod args;
mod cloud;
mod compat;
mod config;
mod depots;
mod install;
mod library;
mod social;
mod workshop;

pub(crate) use args::*;
pub(crate) use cloud::*;
pub(crate) use compat::*;
pub(crate) use config::*;
pub(crate) use depots::*;
pub(crate) use install::*;
pub(crate) use library::*;
pub(crate) use social::*;
pub(crate) use workshop::*;

/// Aurelia — a command-line Steam launcher (auth, library, install, launch).
#[derive(Parser)]
#[command(
    name = "aurelia",
    version,
    about,
    long_about = None,
    // Default template, with a "Version: x.y.z" line inserted under the about text.
    help_template = "{before-help}{about-with-newline}Version: {version}\n\n{usage-heading} {usage}\n\n{all-args}{after-help}"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
    /// Emit output (and errors) as JSON. Works with every command.
    #[arg(short = 'j', long, global = true)]
    pub(crate) json: bool,
    /// Increase log verbosity (repeatable: -v, -vv, -vvv). Without it only
    /// warnings and errors are logged. -v adds progress/debug logs and a launched
    /// game's own output; -vv/-vvv unmute the Steam networking stack so a stalled
    /// command shows where it is stuck. `RUST_LOG` / `AURELIA_LOG` override the logs.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub(crate) verbose: u8,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Authenticate with Steam and persist the session.
    Login(LoginArgs),
    /// Describe this build for drivers: commands, flags, JSON events, error types
    /// and exit codes, and the `--json` schema version. Offline. Without `--json` it
    /// prints a PowerShell-style list and error table. Name a command path
    /// (e.g. `capabilities config game`) for its flags, values and help.
    Capabilities {
        /// Command path to describe, e.g. `config game`.
        path: Vec<String>,
    },
    /// List games in your library.
    List {
        /// Only show installed games.
        #[arg(short, long)]
        installed: bool,
        /// Filter by case-insensitive substring of the game name.
        #[arg(short, long)]
        search: Option<String>,
        /// Only show games in the named collection (by name or id). Static
        /// collections only — dynamic (filter-based) ones can't be resolved offline.
        #[arg(short = 'c', long)]
        collection: Option<String>,
        /// Show an ONLINE column indicating whether each game appears to require
        /// an online connection (inferred from Steam store categories). This
        /// fetches PICS appinfo per game, so it is slower than a plain listing.
        #[arg(short = 'o', long)]
        online: bool,
        /// Compute `update_available` for installed games by comparing local and
        /// remote depot manifests (applies to owned *and* Family-Shared games).
        /// Requires a connection and a manifest fetch per game, so it is slower
        /// than a plain listing; off by default.
        #[arg(short = 'u', long)]
        check_updates: bool,
    },
    /// Show detailed information about a game.
    Info {
        /// One or more app ids. Multiple ids are fetched over a *single* Steam
        /// logon (one batched StoreBrowse call) — far cheaper than running `info`
        /// once per id. With `--json`, one id yields an object and several yield an
        /// array.
        #[arg(required = true)]
        app_ids: Vec<u32>,
        /// Also show storefront-only fields that have no CM-protocol source:
        /// system requirements, Metacritic, website and store genres/categories.
        /// One extra HTTPS request per app.
        #[arg(short = 'e', long)]
        extended: bool,
        /// Bypass the local metadata cache and fetch fresh data from Steam.
        /// By default `info` serves cached store metadata (TTL via
        /// `AURELIA_INFO_CACHE_TTL`, default 6h) to avoid a Steam logon per call.
        #[arg(short = 'n', long)]
        no_cache: bool,
        /// Steam API language name for store text (descriptions, requirements).
        /// Defaults to the `aurelia config language` setting, or English.
        #[arg(short = 'l', long = "lang", alias = "language")]
        lang: Option<String>,
        /// Price country code; defaults to config, locale, US.
        #[arg(short = 'c', long, alias = "cc")]
        country: Option<String>,
    },
    /// Show a game's price, optionally across regions.
    Price {
        app_id: u32,
        /// Country codes to compare, comma-separated.
        #[arg(short = 'r', long, value_delimiter = ',')]
        compare: Vec<String>,
        /// Country code for a single-region lookup.
        #[arg(short = 'c', long, alias = "cc")]
        country: Option<String>,
        /// Steam API language name for the game's name.
        #[arg(short = 'l', long = "lang", alias = "language")]
        lang: Option<String>,
    },
    /// Search the store by title.
    Search {
        term: String,
        /// Maximum results.
        #[arg(short = 'n', long, default_value_t = 20)]
        count: u32,
        /// Country code for prices.
        #[arg(short = 'c', long, alias = "cc")]
        country: Option<String>,
        /// Steam API language name for names.
        #[arg(short = 'l', long = "lang", alias = "language")]
        lang: Option<String>,
    },
    /// Discounted and top-selling games in a region.
    Deals {
        /// Which slice of the store to list.
        #[arg(short = 's', long, value_enum, default_value_t = DealsScopeArg::DiscountedTopSellers)]
        scope: DealsScopeArg,
        /// Minimum discount percent for specials.
        #[arg(short = 'm', long, default_value_t = 1)]
        min_discount: i32,
        /// Maximum results.
        #[arg(short = 'n', long, default_value_t = 20)]
        count: i32,
        /// Result offset for paging.
        #[arg(short = 'o', long, default_value_t = 0)]
        start: i32,
        /// Country code for prices.
        #[arg(short = 'c', long, alias = "cc")]
        country: Option<String>,
        /// Steam API language name for names.
        #[arg(short = 'l', long = "lang", alias = "language")]
        lang: Option<String>,
    },
    /// Current player count for an app.
    Players { app_id: u32 },
    /// Store-wide sales and events currently running.
    Events {
        /// Country code the events apply to.
        #[arg(short = 'c', long, alias = "cc")]
        country: Option<String>,
    },
    /// Recent news (storefront web; no CM equivalent).
    News {
        app_id: u32,
        /// Maximum items.
        #[arg(short = 'n', long, default_value_t = 10)]
        count: u32,
        /// Characters of body text to keep.
        #[arg(short = 'm', long, default_value_t = 300)]
        max_length: u32,
    },
    /// User reviews, paged (storefront web, no CM equivalent).
    Reviews(ReviewsArgs),
    /// Profile by SteamID64, /profiles/ URL, friend name, `me`.
    User { user: String },
    /// Your wishlist, or a friend's.
    #[command(args_conflicts_with_subcommands = true)]
    Wishlist {
        /// Whose wishlist; defaults to yours.
        user: Option<String>,
        /// Maximum items.
        #[arg(short = 'n', long, default_value_t = 50)]
        count: usize,
        /// Skip this many items first.
        #[arg(short = 'o', long, default_value_t = 0)]
        offset: usize,
        /// Country code for prices.
        #[arg(short = 'c', long, alias = "cc")]
        country: Option<String>,
        /// Steam API language name for names.
        #[arg(short = 'l', long = "lang", alias = "language")]
        lang: Option<String>,
        #[command(subcommand)]
        command: Option<WishlistCommand>,
    },
    /// Games similar to a given app.
    Similar {
        app_id: u32,
        /// Maximum results.
        #[arg(short = 'n', long, default_value_t = 10)]
        count: i32,
        /// Country code for prices.
        #[arg(short = 'c', long, alias = "cc")]
        country: Option<String>,
        /// Steam API language name for names.
        #[arg(short = 'l', long = "lang", alias = "language")]
        lang: Option<String>,
    },
    /// Download and install a game.
    Install(InstallArgs),
    /// List the Steam library folders games can be installed into (one per
    /// drive/location). Use `--json` for tooling.
    Libraries,
    /// List installed games with an update available, or update one by app id.
    Update {
        /// Game to update. Omit to list every installed game that needs an update.
        app_id: Option<u32>,
        /// Update even if the game is pinned (see `aurelia pin`), overriding the lock.
        #[arg(short = 'f', long)]
        force: bool,
    },
    /// Launch a game and wait for it to exit.
    Play {
        app_id: u32,
        /// Force a specific Proton/Wine runner (Linux only; implies Windows target).
        #[arg(short, long)]
        proton: Option<String>,
        /// Run the Windows executable directly with no Proton/Wine layer.
        /// Always implied when running on Windows.
        #[arg(short, long)]
        windows: bool,
        /// Route this launch through the luxtorpeda native-engine plugin (Linux only).
        /// Installs the plugin on first use; see `aurelia luxtorpeda`.
        #[arg(short = 'n', long, conflicts_with_all = ["proton", "windows"])]
        native_engine: bool,
        /// Wrap this launch through the umu-launcher plugin (Proton via `umu-run`,
        /// Linux only). Installs the plugin on first use; see `aurelia umu`.
        /// Combine with `--proton` to pick the Proton build umu runs.
        #[arg(short = 'u', long, conflicts_with_all = ["windows", "native_engine"])]
        umu: bool,
        /// Wrap this launch with a specific launch script, overriding the per-game
        /// config and the auto-detected `<script_dir>/<app_id>.sh`. The script runs
        /// with the resolved launch command as its arguments. See `aurelia scripts`.
        #[arg(short = 'x', long, value_name = "PATH")]
        script: Option<PathBuf>,
        /// Bypass all launch scripts for this launch (ignore the per-game config and
        /// any auto-detected script).
        #[arg(short = 'X', long, conflicts_with = "script")]
        no_script: bool,
        /// Run with real Steam integration instead of standalone mode: bridge to the
        /// host Steam client (started silently if not running) so Steamworks online
        /// features work. Implied for Family-Shared games, which require it.
        #[arg(short = 's', long)]
        steam: bool,
        /// Run this installed build (`linux` native or `windows` via Proton),
        /// overriding auto-detection for this launch. See `aurelia launch-targets`.
        #[arg(short = 't', long, conflicts_with = "native_engine")]
        platform: Option<PlatformArg>,
        /// Steam integration for a Windows build, for this launch only: `runtime`
        /// (in-Wine Steam), `host` (bridge to host Steam) or `none` (standalone).
        /// Overrides the configured Steam-runtime policy.
        #[arg(short = 'm', long)]
        steam_mode: Option<SteamModeArg>,
        /// Start this entry from `aurelia launch-options`, by its id, instead of
        /// the default one.
        #[arg(long, value_name = "ID")]
        launch_option: Option<String>,
        /// Skip the automatic update check and install before launching.
        #[arg(short = 'N', long = "no-update", alias = "noupdate")]
        no_update: bool,
        /// Launch without pre-compiling Vulkan shaders first (like Steam's "Skip").
        /// Shaders then compile while the game runs.
        #[arg(short = 'k', long)]
        skip_shaders: bool,
        /// How long to wait for a game started through the in-Wine Steam to appear,
        /// in seconds (default 300). Time Steam spends processing Vulkan shaders
        /// doesn't count.
        #[arg(short = 'L', long, value_name = "SECS", value_parser = clap::value_parser!(u64).range(1..))]
        launch_timeout: Option<u64>,
    },
    /// Pre-compile a game's Vulkan shaders from its Steam pipeline cache
    /// (`steamapps/shadercache/<appid>`), as Steam's "Processing Vulkan shaders"
    /// does. `play` does this automatically unless `config shader-precache off`.
    Shaders {
        app_id: u32,
        /// Replay again even if nothing changed since the last run.
        #[arg(short, long)]
        force: bool,
    },
    /// List the ways an installed game can be launched: each installed build
    /// (native Linux, native macOS or Windows) and, for Windows, each available Steam
    /// mode, with the `play` flags selecting it and which one plain `play` would pick.
    LaunchTargets {
        app_id: u32,
        /// Compute the default as for `play --steam`.
        #[arg(short = 's', long)]
        steam: bool,
    },
    /// Stop a running game previously launched with `aurelia play`.
    Stop {
        /// App id to stop. Omit to list the games Aurelia is tracking as running.
        app_id: Option<u32>,
        /// Force-kill the game immediately (SIGKILL) instead of asking it to exit
        /// gracefully first. Use when a game is hung and ignores a normal stop.
        #[arg(short = 'f', long)]
        force: bool,
    },
    /// List the games Aurelia is currently running.
    Running,
    /// Uninstall a game.
    Uninstall {
        app_id: u32,
        /// Also delete the game's Wine prefix / compat data.
        #[arg(short = 'd', long)]
        delete_prefix: bool,
    },
    /// Verify the integrity of an installed game.
    Verify { app_id: u32 },
    /// List games installed in more than one Steam library.
    ///
    /// A duplicate leaves two appmanifests disagreeing about what is installed,
    /// which makes the game report an available update on every check. Prints the
    /// redundant copies and the commands to remove them; deletes nothing itself.
    Duplicates,
    /// Report whether a game is installed and its files are present on disk.
    Available { app_id: u32 },
    /// Verify a game's DRM tickets headlessly.
    Drm { app_id: u32 },
    /// Show account details for the logged-in user.
    Account,
    /// List a game's DLC.
    Dlc {
        app_id: u32,
        /// Also list retired DLC the store no longer has.
        #[arg(long)]
        all: bool,
    },
    /// Enable an installed DLC for its base game.
    Enable {
        app_id: u32,
        /// Stop Steam while applying the change, then restart it, so the running
        /// client picks it up. Steam reads DLC state only at startup.
        #[arg(short = 'r', long)]
        restart_steam: bool,
    },
    /// Disable a DLC for its base game.
    Disable {
        app_id: u32,
        /// Stop Steam while applying the change, then restart it.
        #[arg(short = 'r', long)]
        restart_steam: bool,
    },
    /// Show the logged-in user's achievements for a game.
    Achievements {
        app_id: u32,
        /// Steam API language name. Defaults to the `aurelia config language`
        /// setting, or English.
        #[arg(short, long, alias = "language")]
        lang: Option<String>,
    },
    /// Download a game's cover/header artwork to the local image cache.
    Image {
        app_id: u32,
        /// Write the image to this path instead of the cache directory.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Re-download even if a cached copy already exists.
        #[arg(short, long)]
        force: bool,
        /// List every store asset URL instead of downloading.
        #[arg(short = 'l', long)]
        list: bool,
        /// With `--list`: HEAD-check each URL.
        #[arg(short = 'p', long, requires = "list")]
        probe: bool,
    },
    /// Dump raw PICS appinfo sections (maintainer tool).
    #[command(hide = true)]
    Appinfo { app_id: u32 },
    /// Dump the store tag vocabulary (maintainer tool).
    #[command(hide = true)]
    Tags {
        /// Print Rust source for `tags_table.rs`.
        #[arg(short = 'd', long)]
        dump: bool,
    },
    /// Move an installed game to a different Steam library folder
    Move {
        app_id: u32,
        /// Destination Steam library folder (its root, containing `steamapps/`),
        /// e.g. `D:\SteamLibrary`. Must already be a Steam library.
        library: PathBuf,
        /// Stop Steam for the duration of the move and restart it afterward.
        /// Steam overwrites its data files on exit, so moving while it runs is
        /// unsafe; without this, the move refuses to run while Steam is open.
        #[arg(short = 'r', long)]
        restart_steam: bool,
    },
    /// Relink an install to a different Steam library.
    Relink {
        app_id: u32,
        /// Destination Steam library root (containing `steamapps/`).
        library: PathBuf,
        /// Stop Steam for the duration and restart it afterward.
        #[arg(short = 'r', long)]
        restart_steam: bool,
    },
    /// Register an existing on-disk install with Steam.
    Import {
        app_id: u32,
        /// Steam library root whose `steamapps/common/<installdir>` holds the files.
        library: PathBuf,
        /// Depot platform whose files are present. Defaults to the current OS.
        #[arg(short, long)]
        platform: Option<PlatformArg>,
        /// Stop Steam for the duration and restart it afterward.
        #[arg(short = 'r', long)]
        restart_steam: bool,
    },
    /// List available beta branches for a game.
    Branches { app_id: u32 },
    /// Switch a game to a different branch.
    SetBranch { app_id: u32, branch: String },
    /// Swap an installed game's language depots.
    SetLanguage {
        app_id: u32,
        /// Steam language name, e.g. `german`.
        language: String,
    },
    /// List depots for a game.
    Depots(DepotsArgs),
    /// List each depot's current manifest id per branch (version discovery for
    /// `downgrade`). Steam only exposes current ids — older ones live on SteamDB.
    Manifests {
        app_id: u32,
        /// Only show this depot.
        #[arg(short = 'd', long)]
        depot: Option<u32>,
    },
    /// Install a specific (usually older) depot manifest and pin it — a downgrade.
    Downgrade(DowngradeArgs),
    /// Pin a game to its currently-installed manifests, locking Aurelia's updates.
    Pin { app_id: u32 },
    /// Remove a game's version pin (unlock Aurelia's updates).
    Unpin { app_id: u32 },
    /// List a game's launch options.
    LaunchOptions { app_id: u32 },
    /// Manage Steam Cloud saves for a game.
    Cloud {
        #[command(subcommand)]
        command: CloudCommand,
    },
    /// Manage Steam Workshop items for a game.
    Workshop {
        #[command(subcommand)]
        command: WorkshopCommand,
    },
    /// Download and manage Proton/Wine runtimes.
    Proton {
        #[command(subcommand)]
        command: ProtonCommand,
    },
    /// Install, repair, or inspect the master Windows Steam runtime prefix.
    SteamRuntime {
        #[command(subcommand)]
        command: SteamRuntimeCommand,
    },
    /// Manage the optional luxtorpeda native-engine plugin.
    Luxtorpeda {
        #[command(subcommand)]
        command: LuxtorpedaCommand,
    },
    /// Manage the optional umu-launcher plugin (Proton via umu).
    Umu {
        #[command(subcommand)]
        command: UmuCommand,
    },
    /// Manage per-game launch scripts.
    Scripts {
        #[command(subcommand)]
        command: ScriptsCommand,
    },
    /// Manage Steam library collections (categories/groups).
    Collections {
        #[command(subcommand)]
        command: CollectionsCommand,
    },
    /// List friends, search for a SteamID, or add/remove friends.
    Friends {
        #[command(subcommand)]
        command: Option<FriendsCommand>,
    },
    /// Send and read direct chat messages.
    Chat {
        #[command(subcommand)]
        command: ChatCommand,
    },
    /// View your Steam inventory for a game.
    Inventory {
        app_id: u32,
        /// Inventory context id (default 2; Steam community items use 6).
        #[arg(short = 'c', long, default_value_t = 2)]
        context: u32,
    },
    /// Show your Steam Wallet balance.
    Wallet,
    /// Steam Community Market: prices, search, and your listings.
    Market {
        #[command(subcommand)]
        command: MarketCommand,
    },
    /// Inspect launcher configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Clear the stored session.
    Logout,
    /// Run the background session daemon serving other commands over a local socket.
    Daemon {
        /// Override the socket/pipe path (also settable via `AURELIA_DAEMON_SOCKET`).
        #[arg(short = 's', long)]
        socket: Option<String>,
        #[command(subcommand)]
        command: Option<DaemonCommand>,
    },
    /// Kill all running aurelia processes, including the session daemon.
    Kill,
}

#[derive(Subcommand)]
pub(crate) enum DaemonCommand {
    /// Stop running aurelia daemon(s). With a PID, stop only that daemon.
    Stop {
        /// The daemon PID to stop (from `aurelia daemon list`). Omit to stop all.
        pid: Option<u32>,
    },
    /// List running aurelia daemon(s) with their PID.
    List,
}

/// `aurelia login` flags.
#[derive(clap::Args)]
pub(crate) struct LoginArgs {
    /// Steam account name (prompted if omitted).
    #[arg(short, long)]
    pub(crate) username: Option<String>,
    /// Password (prompted, or AURELIA_PASSWORD).
    #[arg(short, long)]
    pub(crate) password: Option<String>,
    /// Steam Guard code, if known.
    #[arg(short, long)]
    pub(crate) guard: Option<String>,
    /// Log in via Steam Mobile QR code.
    #[arg(short = 'q', long, conflicts_with_all = ["username", "password", "guard", "code"])]
    pub(crate) qr: bool,
    /// With `--qr`: give up after this many seconds without approval (default 600).
    /// With `--json`, a `cancel` line or EOF on stdin also ends the wait.
    #[arg(short = 't', long, value_name = "SECS", requires = "qr", value_parser = clap::value_parser!(u64).range(1..))]
    pub(crate) timeout: Option<u64>,
    /// Type the Guard code instead of app approval.
    #[arg(short = 'c', long, visible_alias = "pin", conflicts_with = "guard")]
    pub(crate) code: bool,
    /// [EXPERIMENTAL] Browser OpenID identity check (no session token).
    #[arg(short = 'o', long, hide = true, conflicts_with_all = ["username", "password", "guard", "qr", "code"])]
    pub(crate) openid: bool,
    /// [EXPERIMENTAL] Store a web token from steamcommunity.com/chat/clientjstoken
    /// (or AURELIA_WEB_TOKEN) for web-only commands; expires in ~24h.
    #[arg(short = 'w', long, hide = true, num_args = 0..=1, conflicts_with_all = ["username", "password", "guard", "qr", "code", "openid"])]
    pub(crate) web_token: Option<Option<String>>,
    /// Report session health without logging in.
    #[arg(short = 'H', long, conflicts_with_all = ["username", "password", "guard", "qr", "code", "openid", "web_token", "reconnect"])]
    pub(crate) health: bool,
    /// Re-establish the daemon session (daemon required).
    #[arg(short = 'r', long, conflicts_with_all = ["username", "password", "guard", "qr", "code", "openid", "web_token", "health"])]
    pub(crate) reconnect: bool,
}
