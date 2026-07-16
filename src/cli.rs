//! clap CLI type definitions (parsed command surface).

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use aurelia::core::models::DepotPlatform;

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
    #[arg(long, global = true)]
    pub(crate) json: bool,
    /// Increase log verbosity (repeatable: -v, -vv, -vvv). Unmutes the Steam
    /// networking stack so a stalled command shows where it is stuck.
    /// `RUST_LOG` / `AURELIA_LOG` override this.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub(crate) verbose: u8,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Authenticate with Steam and persist the session.
    Login {
        /// Steam account name. Prompted if omitted.
        #[arg(short, long)]
        username: Option<String>,
        /// Account password. Prompted securely if omitted (or set AURELIA_PASSWORD).
        #[arg(short, long)]
        password: Option<String>,
        /// Steam Guard code (email or mobile authenticator), if known up front.
        #[arg(short, long)]
        guard: Option<String>,
        /// Log in by scanning a QR code with the Steam Mobile app (no
        /// username/password needed). Renders the QR in the terminal.
        #[arg(long, conflicts_with_all = ["username", "password", "guard", "code"])]
        qr: bool,
        /// Enter the Steam Guard code interactively when prompted, instead of
        /// approving the login in the Steam Mobile app. (Alias: --pin)
        #[arg(long, visible_alias = "pin", conflicts_with = "guard")]
        code: bool,
        /// Verify your identity in the browser on the official Steam sign-in page
        /// (OpenID) — the password is only ever typed on steamcommunity.com.
        /// Identity-only: Steam issues no session token over OpenID, so commands
        /// that need a session still require one of the other login methods.
        #[arg(long, conflicts_with_all = ["username", "password", "guard", "qr", "code"])]
        openid: bool,
        /// Store a browser web token to enable the web-surface commands
        /// (inventory, wallet, market listings) without a client login: open
        /// https://steamcommunity.com/chat/clientjstoken in a signed-in browser
        /// (e.g. right after `login --openid`) and paste the JSON shown — as this
        /// flag's value, or when prompted if the value is omitted. Web-only and
        /// short-lived (~24h); it cannot replace a full login.
        #[arg(long, num_args = 0..=1, conflicts_with_all = ["username", "password", "guard", "qr", "code", "openid"])]
        web_token: Option<Option<String>>,
        /// Report the current session health (authenticated? which account?) without
        /// logging in. Reflects the daemon's shared session when one is in use.
        #[arg(long, conflicts_with_all = ["username", "password", "guard", "qr", "code", "openid", "web_token", "reconnect"])]
        health: bool,
        /// Tear down and re-establish the daemon's shared session from the stored
        /// token — use after the live connection dropped. Requires a running daemon.
        #[arg(long, conflicts_with_all = ["username", "password", "guard", "qr", "code", "openid", "web_token", "health"])]
        reconnect: bool,
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
        #[arg(long)]
        collection: Option<String>,
        /// Show an ONLINE column indicating whether each game appears to require
        /// an online connection (inferred from Steam store categories). This
        /// fetches PICS appinfo per game, so it is slower than a plain listing.
        #[arg(long)]
        online: bool,
        /// Compute `update_available` for installed games by comparing local and
        /// remote depot manifests (applies to owned *and* Family-Shared games).
        /// Requires a connection and a manifest fetch per game, so it is slower
        /// than a plain listing; off by default.
        #[arg(long)]
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
        /// system requirements, Metacritic, website, store genres/categories and
        /// SteamSpy user tags. This makes additional HTTPS storefront requests.
        #[arg(long)]
        extended: bool,
        /// Bypass the local metadata cache and fetch fresh data from Steam.
        /// By default `info` serves cached store metadata (TTL via
        /// `AURELIA_INFO_CACHE_TTL`, default 6h) to avoid a Steam logon per call.
        #[arg(long)]
        no_cache: bool,
        /// Steam API language name for store text (descriptions, requirements).
        /// Defaults to the `aurelia config language` setting, or English.
        #[arg(short = 'l', long = "lang")]
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
        #[arg(long)]
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
        #[arg(long, conflicts_with_all = ["proton", "windows"])]
        native_engine: bool,
        /// Wrap this launch through the umu-launcher plugin (Proton via `umu-run`,
        /// Linux only). Installs the plugin on first use; see `aurelia umu`.
        /// Combine with `--proton` to pick the Proton build umu runs.
        #[arg(long, conflicts_with_all = ["windows", "native_engine"])]
        umu: bool,
        /// Wrap this launch with a specific launch script, overriding the per-game
        /// config and the auto-detected `<script_dir>/<app_id>.sh`. The script runs
        /// with the resolved launch command as its arguments. See `aurelia scripts`.
        #[arg(long, value_name = "PATH")]
        script: Option<PathBuf>,
        /// Bypass all launch scripts for this launch (ignore the per-game config and
        /// any auto-detected script).
        #[arg(long, conflicts_with = "script")]
        no_script: bool,
        /// Run with real Steam integration instead of standalone mode: bridge to the
        /// host Steam client (started silently if not running) so Steamworks online
        /// features work. Implied for Family-Shared games, which require it.
        #[arg(long)]
        steam: bool,
        /// Skip the automatic update check and install before launching.
        #[arg(long)]
        noupdate: bool,
    },
    /// Stop a running game previously launched with `aurelia play`.
    Stop {
        /// App id to stop. Omit to list the games Aurelia is tracking as running.
        app_id: Option<u32>,
        /// Force-kill the game immediately (SIGKILL) instead of asking it to exit
        /// gracefully first. Use when a game is hung and ignores a normal stop.
        #[arg(long)]
        force: bool,
    },
    /// List the games Aurelia is currently running.
    Running,
    /// Uninstall a game.
    Uninstall {
        app_id: u32,
        /// Also delete the game's Wine prefix / compat data.
        #[arg(long)]
        delete_prefix: bool,
    },
    /// Verify the integrity of an installed game.
    Verify { app_id: u32 },
    /// Report whether a game is installed and its files are present on disk.
    Available { app_id: u32 },
    /// Show account details for the logged-in user.
    Account,
    /// List a game's DLC.
    Dlc { app_id: u32 },
    /// Enable an installed DLC for its base game.
    Enable {
        app_id: u32,
        /// Stop Steam while applying the change, then restart it, so the running
        /// client picks it up (Windows). Steam reads DLC state only at startup.
        #[arg(long)]
        restart_steam: bool,
    },
    /// Disable a DLC for its base game.
    Disable {
        app_id: u32,
        /// Stop Steam while applying the change, then restart it (Windows).
        #[arg(long)]
        restart_steam: bool,
    },
    /// Show the logged-in user's achievements for a game.
    Achievements {
        app_id: u32,
        /// Steam API language name. Defaults to the `aurelia config language`
        /// setting, or English.
        #[arg(short, long)]
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
        #[arg(long)]
        restart_steam: bool,
    },
    /// Relink an install to a different Steam library.
    Relink {
        app_id: u32,
        /// Destination Steam library root (containing `steamapps/`).
        library: PathBuf,
        /// Stop Steam for the duration and restart it afterward.
        #[arg(long)]
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
        #[arg(long)]
        restart_steam: bool,
    },
    /// List available beta branches for a game.
    Branches { app_id: u32 },
    /// Switch a game to a different branch.
    SetBranch { app_id: u32, branch: String },
    /// List depots for a game.
    Depots { app_id: u32 },
    /// List each depot's current manifest id per branch (version discovery for
    /// `downgrade`). Steam only exposes current ids — older ones live on SteamDB.
    Manifests {
        app_id: u32,
        /// Only show this depot.
        #[arg(long)]
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
        #[arg(long, default_value_t = 2)]
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
        #[arg(long)]
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

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    /// Print the current launcher configuration as JSON.
    Show,
    /// List detected Proton/Wine runtimes.
    Protons,
    /// View or set the Steam presence the daemon announces for friends/chat.
    Presence {
        /// `online`, or `offline` (invisible: you appear offline but still sync
        /// friends and receive chat). Omit to print the current setting.
        mode: Option<ChatPresenceArg>,
    },
    /// View or set the default Steam API language.
    Language {
        /// The default Steam API language name (e.g. `german`, `french`,
        /// `schinese`) used by `aurelia achievements` when `--lang` is not
        /// given. Omit the value to print the current setting.
        lang: Option<String>,
    },
    /// View or set the Wine/Proton runner that hosts the Windows Steam runtime.
    ///
    /// Used by `steam-runtime install`/`repair` to drive `SteamSetup.exe` and the
    /// background Steam client under a bare Wine. Accepts an installed runtime name
    /// (see `aurelia proton list`) such as `GE-Proton9-20` or `experimental`, or an
    /// absolute path to a Wine build. A Proton tree is fine — its bundled bare Wine
    /// is used automatically. Pass an empty value to clear it.
    SteamRuntimeRunner {
        /// Installed runtime name or absolute Wine path. Omit to print the current
        /// setting; pass `""` to clear it.
        runner: Option<String>,
    },
    /// View or set the network proxy used for all HTTP(S) communication.
    ///
    /// Applies to the Steam web endpoints, depot downloads, and Proton/plugin
    /// release lookups. Takes effect on the next command (and requires restarting
    /// the session daemon). An explicit `HTTP(S)_PROXY`/`ALL_PROXY` environment
    /// variable still overrides this. The Steam CM connection is not proxied.
    Proxy {
        /// Proxy URL, e.g. `http://host:8080`, `http://user:pass@host:8080`, or
        /// `socks5://host:1080`. Omit to print the current setting.
        url: Option<String>,
        /// Comma-separated hosts/domains that bypass the proxy (`NO_PROXY`), e.g.
        /// `localhost,127.0.0.1,.internal`.
        #[arg(long, value_name = "LIST")]
        no_proxy: Option<String>,
        /// Clear the configured proxy (revert to a direct connection).
        #[arg(long, conflicts_with_all = ["url", "no_proxy"])]
        clear: bool,
    },
    /// View or set per-game launch settings (Proton version, platform).
    Game {
        app_id: u32,
        /// Set the Proton/Wine version this game launches with. Use a name from
        /// `aurelia proton list` (installed). Overrides the global default.
        #[arg(long)]
        proton: Option<String>,
        /// Clear the per-game Proton version (fall back to the global default).
        #[arg(long, conflicts_with = "proton")]
        clear_proton: bool,
        /// Force the game's platform target (`windows` runs through Proton on Linux).
        #[arg(long)]
        platform: Option<PlatformArg>,
        /// Route this game through the luxtorpeda native-engine plugin (Linux only;
        /// requires `aurelia luxtorpeda enable`).
        #[arg(long)]
        native_engine: bool,
        /// Clear the luxtorpeda routing (back to Aurelia's normal native/Proton selection).
        #[arg(long, conflicts_with = "native_engine")]
        no_native_engine: bool,
        /// Route this game through the umu-launcher plugin (Proton via umu; Linux only;
        /// requires `aurelia umu enable`).
        #[arg(long, conflicts_with_all = ["native_engine", "no_native_engine"])]
        umu: bool,
        /// Clear the umu routing (back to Aurelia's normal native/Proton selection).
        #[arg(long, conflicts_with = "umu")]
        no_umu: bool,
        /// Set a per-game launch script that wraps the resolved launch command. See
        /// `aurelia scripts`. Overrides the auto-detected `<script_dir>/<app_id>.sh`.
        #[arg(long, value_name = "PATH")]
        launch_script: Option<PathBuf>,
        /// Clear the per-game launch script (falls back to the auto-detected script).
        #[arg(long, conflicts_with = "launch_script")]
        no_launch_script: bool,
        /// Use the self-contained Windows Steam runtime for this game: `on` starts the
        /// master Steam client in Wine to satisfy Steamworks/DRM handshakes without the
        /// host Steam client. Requires `aurelia config steam-runtime-runner` and
        /// `aurelia steam-runtime install`. `auto` is the default (off).
        #[arg(long, value_name = "auto|on|off")]
        steam_runtime: Option<SteamRuntimeArg>,
        /// How the master Steam prefix backs this game: `shared` runs it in the master
        /// prefix directly; `per-game` copies Steam into the game's own prefix.
        #[arg(long, value_name = "shared|per-game")]
        steam_prefix_mode: Option<SteamPrefixModeArg>,
    },
}

#[derive(Subcommand)]
pub(crate) enum ProtonCommand {
    /// List installable runtimes (Valve + GE) and what's already installed.
    List {
        /// Only show what's installed on disk (skips the GitHub/Valve lookup).
        #[arg(long)]
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
}

#[derive(Subcommand)]
pub(crate) enum SteamRuntimeCommand {
    /// Download SteamSetup.exe (if needed) and install Steam into the master prefix.
    /// Requires `steam_runtime_runner` to be configured.
    Install,
    /// Stop Steam, back up the master prefix (keeping one `.bak`), then reinstall.
    /// Requires `steam_runtime_runner` to be configured.
    Repair,
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
        #[arg(long, conflicts_with = "path")]
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
        #[arg(long, conflicts_with = "path")]
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
        #[arg(long)]
        force: bool,
    },
    /// Print the resolved launch-script path for a game and its contents.
    Show { app_id: u32 },
    /// Delete the dir-based launch script for a game.
    Remove { app_id: u32 },
}

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
    },
    /// Pull then push — reconcile local and Steam collections (needs login).
    /// This changes your real Steam library — confirm or pass `--yes`.
    Sync {
        /// Skip the confirmation prompt. Required in `--json` mode.
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum FriendsCommand {
    /// List your friends (the default when no subcommand is given).
    List,
    /// Resolve a SteamID from a SteamID64, profile URL, or custom (vanity) URL/name.
    /// Read-only; no login required.
    Search { query: String },
    /// Send a friend request. Accepts a SteamID64, profile URL, or custom URL/name.
    Add { query: String },
    /// Remove a friend, or cancel/decline a pending request (by SteamID64).
    Remove { steamid: u64 },
}

#[derive(Subcommand)]
pub(crate) enum MarketCommand {
    /// Look up an item's market price (no login required).
    Price {
        app_id: u32,
        /// Exact market hash name (case-sensitive), e.g. "Mann Co. Supply Crate Key".
        name: String,
        /// Steam currency id (1=USD, 2=GBP, 3=EUR, …).
        #[arg(long, default_value_t = 1)]
        currency: u32,
    },
    /// Search the Community Market (no login required).
    Search {
        /// Free-text query (optional).
        query: Option<String>,
        /// Restrict to one game by app id.
        #[arg(long)]
        app_id: Option<u32>,
        /// Maximum results to return.
        #[arg(long, default_value_t = 20)]
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
    /// Show recent messages exchanged with a friend (by SteamID64).
    History {
        steamid: u64,
        /// How many recent messages to fetch.
        #[arg(long, default_value_t = 20)]
        count: u32,
    },
    /// Open an interactive live chat with a friend: type lines to send, incoming
    /// messages stream in. End with Ctrl-D (Ctrl-Z on Windows). With `--json`,
    /// emits one JSON event per line and reads message text from stdin lines.
    Open { steamid: u64 },
}

#[derive(Subcommand)]
pub(crate) enum CloudCommand {
    /// Sync a game's Steam Cloud saves with the local save directory. With
    /// neither flag it syncs down then up; `--down`/`--up` restrict the direction.
    Sync {
        app_id: u32,
        /// Only upload local saves to Steam.
        #[arg(long, conflicts_with = "down")]
        up: bool,
        /// Only download saves from Steam.
        #[arg(long, conflicts_with = "up")]
        down: bool,
        /// Local save directory. Defaults to Aurelia's managed cloud root.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Resolve diverged saves by taking this side (`cloud` or `local`) instead
        /// of reporting them. Omit to only detect conflicts and leave both copies.
        #[arg(long, value_enum)]
        resolve: Option<CloudResolve>,
    },
    /// List a game's Steam Cloud files (name, size, modified time).
    List { app_id: u32 },
}

#[derive(Subcommand)]
pub(crate) enum WorkshopCommand {
    /// Browse/search a game's Workshop to discover items to subscribe to or install.
    Browse {
        app_id: u32,
        /// Filter by search text (free-text match on title/description).
        #[arg(short, long)]
        search: Option<String>,
        /// Sort order.
        #[arg(long, value_enum, default_value_t = WorkshopSort::Trend)]
        sort: WorkshopSort,
        /// Number of results per page (1–100).
        #[arg(long, default_value_t = 20)]
        count: u32,
        /// Pagination cursor; use a previous page's `next_cursor` (`*` = first page).
        #[arg(long, default_value = "*")]
        cursor: String,
        /// Only items carrying this tag (repeatable).
        #[arg(long = "tag")]
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
        #[arg(long)]
        no_recurse: bool,
    },
    /// Remove one or more installed Workshop items (or collections).
    Uninstall {
        #[arg(required = true)]
        ids: Vec<u64>,
        /// Uninstall only the given ids; do not expand collections to their members.
        #[arg(long)]
        no_recurse: bool,
    },
    /// Subscribe to one or more Workshop items (or collections).
    Subscribe {
        #[arg(required = true)]
        ids: Vec<u64>,
        /// Also download the content after subscribing.
        #[arg(long)]
        install: bool,
        /// Subscribe only to the given ids; do not expand collections to their members.
        #[arg(long)]
        no_recurse: bool,
    },
    /// Unsubscribe from one or more Workshop items (or collections).
    Unsubscribe {
        #[arg(required = true)]
        ids: Vec<u64>,
        /// Unsubscribe only from the given ids; do not expand collections.
        #[arg(long)]
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
        #[arg(long, default_value_t = 20)]
        count: i32,
        /// Index of the first comment to fetch (for paging).
        #[arg(long, default_value_t = 0)]
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
    /// client picks up the change (Windows). Without this it only warns.
    #[arg(long)]
    pub(crate) restart_steam: bool,
    /// Don't install — just report the estimated download and on-disk size
    /// (from PICS, no files fetched). Pair with `--json` for tooling.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Steam library folder (drive/location) to install into. A library root
    /// containing a `steamapps` directory, as listed by `aurelia libraries`.
    /// Defaults to the configured `steam_library_path`.
    #[arg(long)]
    pub(crate) library: Option<String>,
}

#[derive(clap::Subcommand)]
pub(crate) enum InstallAction {
    /// List in-flight installs (use --json for tooling).
    List,
    /// Stop a running install by app id.
    Stop { app_id: u32 },
}

/// `aurelia downgrade`: install specific depot manifests (an older version) and
/// pin them. `--depot`/`--manifest` are parallel repeatable lists paired by
/// position; `--manifest <depot>:<manifest>` is an alternative combined form.
#[derive(clap::Args)]
pub(crate) struct DowngradeArgs {
    /// App id to downgrade.
    pub(crate) app_id: u32,
    /// Target depot id (repeatable). Paired by position with a bare `--manifest`.
    #[arg(long = "depot", value_name = "DEPOT_ID")]
    pub(crate) depots: Vec<u32>,
    /// Target manifest id (repeatable). Either a bare id (paired by position with
    /// `--depot`) or the combined `<depot>:<manifest>` form.
    #[arg(long = "manifest", value_name = "MANIFEST_ID")]
    pub(crate) manifests: Vec<String>,
    /// Branch whose build id to record in the appmanifest (default: public).
    #[arg(long)]
    pub(crate) branch: Option<String>,
    /// Password for a protected branch (recorded only; see the downgrade docs).
    #[arg(long)]
    pub(crate) branch_password: Option<String>,
    /// Steam library folder (drive/location) to install into.
    #[arg(long)]
    pub(crate) library: Option<String>,
    /// Verify the install after downloading (integrity pass).
    #[arg(long)]
    pub(crate) verify: bool,
    /// Don't pin after downgrading (Aurelia's update commands may re-upgrade it).
    #[arg(long)]
    pub(crate) no_pin: bool,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum PlatformArg {
    Windows,
    Linux,
}

/// `--steam-runtime auto|on|off`: per-game policy for the self-contained Windows Steam
/// runtime (background master Steam in Wine).
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum SteamRuntimeArg {
    /// Default behavior (currently off).
    Auto,
    /// Start the master Steam client in Wine for this game's Steamworks/DRM handshake.
    On,
    /// Never use the master Steam runtime for this game.
    Off,
}

/// `--steam-prefix-mode shared|per-game`: how the master Steam prefix backs a game.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum SteamPrefixModeArg {
    /// Run the game in the master Steam prefix directly.
    Shared,
    /// Copy/symlink Steam into the game's own prefix.
    PerGame,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum ChatPresenceArg {
    Online,
    Offline,
}

impl From<ChatPresenceArg> for aurelia::core::config::ChatPresence {
    fn from(value: ChatPresenceArg) -> Self {
        match value {
            ChatPresenceArg::Online => aurelia::core::config::ChatPresence::Online,
            ChatPresenceArg::Offline => aurelia::core::config::ChatPresence::Offline,
        }
    }
}

/// Which side wins when `cloud sync` finds a diverged save.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum CloudResolve {
    /// Overwrite the local copy with the cloud copy.
    Cloud,
    /// Overwrite the cloud copy with the local copy.
    Local,
}

impl From<CloudResolve> for aurelia::library::cloud_sync::ConflictPolicy {
    fn from(value: CloudResolve) -> Self {
        match value {
            CloudResolve::Cloud => aurelia::library::cloud_sync::ConflictPolicy::TakeCloud,
            CloudResolve::Local => aurelia::library::cloud_sync::ConflictPolicy::TakeLocal,
        }
    }
}

/// Sort order for `workshop browse`, mapped to an `EPublishedFileQueryType`.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum WorkshopSort {
    /// Trending now (default).
    Trend,
    /// Highest rated overall.
    Popular,
    /// Most recently published.
    Recent,
    /// Most recently updated.
    Updated,
    /// Most subscribed.
    Subscriptions,
    /// Best text-search relevance (use with `--search`).
    Text,
}

impl WorkshopSort {
    pub(crate) fn query_type(self) -> u32 {
        match self {
            WorkshopSort::Trend => 3,
            WorkshopSort::Popular => 0,
            WorkshopSort::Recent => 1,
            WorkshopSort::Updated => 21,
            WorkshopSort::Subscriptions => 9,
            WorkshopSort::Text => 12,
        }
    }
}

/// Thumbs-up/down for `workshop rate`.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum VoteArg {
    Up,
    Down,
}

impl From<PlatformArg> for DepotPlatform {
    fn from(value: PlatformArg) -> Self {
        match value {
            PlatformArg::Windows => DepotPlatform::Windows,
            PlatformArg::Linux => DepotPlatform::Linux,
        }
    }
}
