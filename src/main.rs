use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use aurelia::config::{
    info_cache_ttl, load_info_cache, load_launcher_config, load_library_cache, load_session,
    load_user_configs, save_info_cache,
};
use aurelia::library::{build_game_library, scan_installed_app_info};
use aurelia::models::{
    DepotPlatform, DownloadProgress, DownloadProgressState, DownloadState, LibraryGame,
};
use aurelia::steam_client::{SharedApp, SteamClient, StoreAppInfo};

#[macro_use]
mod output;
mod daemon;
mod proc_admin;

/// Aurelia — a command-line Steam launcher (auth, library, install, launch).
#[derive(Parser)]
#[command(name = "aurelia", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Emit output (and errors) as JSON. Works with every command.
    #[arg(long, global = true)]
    json: bool,
    /// Increase log verbosity (repeatable: -v, -vv, -vvv). Unmutes the Steam
    /// networking stack so a stalled command shows where it is stuck.
    /// `RUST_LOG` / `AURELIA_LOG` override this.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Subcommand)]
enum Command {
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
        /// Report the current session health (authenticated? which account?) without
        /// logging in. Reflects the daemon's shared session when one is in use.
        #[arg(long, conflicts_with_all = ["username", "password", "guard", "qr", "code", "reconnect"])]
        health: bool,
        /// Tear down and re-establish the daemon's shared session from the stored
        /// token — use after the live connection dropped. Requires a running daemon.
        #[arg(long, conflicts_with_all = ["username", "password", "guard", "qr", "code", "health"])]
        reconnect: bool,
    },
    /// Clear the stored session.
    Logout,
    /// List games in your library.
    List {
        /// Only show installed games.
        #[arg(short, long)]
        installed: bool,
        /// Filter by case-insensitive substring of the game name.
        #[arg(short, long)]
        search: Option<String>,
        /// Show an ONLINE column indicating whether each game appears to require
        /// an online connection (inferred from Steam store categories). This
        /// fetches PICS appinfo per game, so it is slower than a plain listing.
        #[arg(long)]
        online: bool,
    },
    /// Show account details for the logged-in user.
    Account,
    /// Download and install a game.
    Install {
        app_id: u32,
        /// Depot platform to install. Auto-detected if omitted.
        #[arg(short, long)]
        platform: Option<PlatformArg>,
        /// When installing a DLC, restart the Steam client afterward so the running
        /// client picks up the change (Windows). Without this it only warns.
        #[arg(long)]
        restart_steam: bool,
        /// Don't install — just report the estimated download and on-disk size
        /// (from PICS, no files fetched). Pair with `--json` for tooling.
        #[arg(long)]
        dry_run: bool,
    },
    /// Uninstall a game.
    Uninstall {
        app_id: u32,
        /// Also delete the game's Wine prefix / compat data.
        #[arg(long)]
        delete_prefix: bool,
    },
    /// Move an installed game to a different Steam library folder, updating
    /// Steam's data so the client recognises the new install path.
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
    /// Relink an install to a different Steam library **without copying** — the
    /// files must already be at the destination (e.g. you moved them by hand).
    Relink {
        app_id: u32,
        /// Destination Steam library root (containing `steamapps/`).
        library: PathBuf,
        /// Stop Steam for the duration and restart it afterward.
        #[arg(long)]
        restart_steam: bool,
    },
    /// Register an existing on-disk install with Steam (writes its appmanifest).
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
    /// Report whether a game is installed and its files are present on disk.
    Available { app_id: u32 },
    /// Verify the integrity of an installed game.
    Verify { app_id: u32 },
    /// Download the latest manifest for an installed game.
    Update { app_id: u32 },
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
    },
    /// Stop a running game previously launched with `aurelia play`.
    Stop {
        /// App id to stop. Omit to list the games Aurelia is tracking as running.
        app_id: Option<u32>,
    },
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
    /// List available beta branches for a game.
    Branches { app_id: u32 },
    /// Switch a game to a different branch.
    SetBranch { app_id: u32, branch: String },
    /// Show detailed information about a game (description, release, reviews, DLC).
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
    },
    /// List a game's DLC (app id and name only).
    Dlc { app_id: u32 },
    /// Show the logged-in user's achievements for a game (with unlock state).
    Achievements {
        app_id: u32,
        /// Language for achievement names/descriptions (Steam API language name).
        #[arg(short, long, default_value = "english")]
        lang: String,
    },
    /// List depots for a game.
    Depots { app_id: u32 },
    /// List a game's launch options (executables/arguments Steam can start it with).
    LaunchOptions { app_id: u32 },
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
    /// Inspect launcher configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Manage Steam Cloud saves for a game.
    Cloud {
        #[command(subcommand)]
        command: CloudCommand,
    },
    /// Manage Steam Workshop items (published files) for a game.
    Workshop {
        #[command(subcommand)]
        command: WorkshopCommand,
    },
    /// Kill all running aurelia processes, including the session daemon.
    Kill,
    /// Run the background session daemon: log in to Steam **once** and serve every
    /// other `aurelia` command over a local socket, so repeated commands never
    /// re-authenticate (avoiding Steam's logon rate limits). Start one per session
    /// (e.g. at Heroic startup); other invocations auto-connect to it.
    ///
    /// With a subcommand (`stop`/`list`) it manages running daemons instead of
    /// starting one.
    Daemon {
        /// Override the socket/pipe path (also settable via `AURELIA_DAEMON_SOCKET`).
        #[arg(long)]
        socket: Option<String>,
        #[command(subcommand)]
        command: Option<DaemonCommand>,
    },
}

#[derive(Subcommand)]
enum DaemonCommand {
    /// Stop running aurelia daemon(s). With a PID, stop only that daemon.
    Stop {
        /// The daemon PID to stop (from `aurelia daemon list`). Omit to stop all.
        pid: Option<u32>,
    },
    /// List running aurelia daemon(s) with their PID.
    List,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Print the current launcher configuration as JSON.
    Show,
    /// List detected Proton/Wine runtimes.
    Protons,
}

#[derive(Subcommand)]
enum CloudCommand {
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
    },
    /// List a game's Steam Cloud files (name, size, modified time).
    List { app_id: u32 },
}

#[derive(Subcommand)]
enum WorkshopCommand {
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

#[derive(Clone, Copy, ValueEnum)]
enum PlatformArg {
    Windows,
    Linux,
}

/// Sort order for `workshop browse`, mapped to an `EPublishedFileQueryType`.
#[derive(Clone, Copy, ValueEnum)]
enum WorkshopSort {
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
    fn query_type(self) -> u32 {
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
enum VoteArg {
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

fn main() {
    // The CLI's top-level async future is large (every command arm) and the Steam
    // connect/auth path is deeply nested; in debug builds this can overflow the OS
    // main thread's default stack (~1 MB on Windows) before any command runs. Run
    // the Tokio runtime on a thread with a generous stack to avoid that.
    let worker = std::thread::Builder::new()
        .name("aurelia-main".to_string())
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(16 * 1024 * 1024)
                .build()
                .expect("failed to build the Tokio runtime");
            runtime.block_on(async_main());
        })
        .expect("failed to spawn the main worker thread");
    worker.join().expect("the main worker thread panicked");
}

/// Whether this invocation must run in **this** process rather than being forwarded
/// to the session daemon.
///
/// Interactive `login` (default credential prompts, `--qr`, or `--code`/`--pin`) needs
/// the real terminal: it reads the username and **masks the password**. Forwarding it
/// would run `rpassword` inside the daemon — a process with no access to the user's tty
/// — so the password could not be hidden and would be echoed in clear text. The daemon
/// reloads the shared session from `session.json` (by mtime) on the next forwarded
/// command, so logging in locally still updates it. `login --health`/`--reconnect`
/// (daemon-oriented) and `login --json` (non-tty, GUI-driven NDJSON) are not interactive
/// and keep forwarding.
///
/// `kill` and `daemon stop|list` manage local OS processes directly and must likewise
/// never forward to (or auto-spawn) the daemon they may be about to terminate.
fn must_run_locally(cli: &Cli) -> bool {
    let interactive_login = !cli.json
        && matches!(
            cli.command,
            Command::Login {
                health: false,
                reconnect: false,
                ..
            }
        );

    interactive_login
        || matches!(
            cli.command,
            Command::Kill | Command::Daemon { command: Some(_), .. }
        )
}

async fn async_main() {
    let cli = Cli::parse();

    // Send tracing/diagnostics to stderr so stdout stays clean for --json output.
    aurelia::infra::logging::init_cli_logging(cli.verbose);

    // Bare `aurelia daemon`: become the shared-session server. Never forwards.
    if let Command::Daemon { socket, command: None } = &cli.command {
        if let Some(path) = socket {
            // SAFETY: single-threaded at this point (set before any worker spawns).
            unsafe { std::env::set_var("AURELIA_DAEMON_SOCKET", path) };
        }
        if let Err(e) = daemon::run_server().await {
            cli_eprintln!("daemon error: {e:#}");
            std::process::exit(1);
        }
        return;
    }

    let local_only = must_run_locally(&cli);

    // Thin client: forward this command to a running daemon (auto-spawning one if
    // needed) so it runs against the single shared Steam session. If no daemon is
    // available, fall through and run locally. `AURELIA_NO_DAEMON` opts out.
    if !local_only && std::env::var_os("AURELIA_NO_DAEMON").is_none() {
        match daemon::client::try_forward().await {
            // Exit the process directly rather than returning: the stdin-forwarding
            // task reads `tokio::io::stdin()` on a blocking thread that never
            // finishes for an interactive (no-EOF) stdin, which would otherwise stall
            // the runtime's shutdown and hang exit.
            Ok(Some(code)) => std::process::exit(code),
            Ok(None) => {}
            Err(e) => tracing::warn!("daemon forwarding failed ({e:#}); running locally"),
        }
    }

    let code = run_and_report(cli).await;
    std::process::exit(code);
}

/// Parse `argv` and run the resulting command, reporting any error the same way the
/// CLI does. Returns the process exit code. This is the daemon's entry point for a
/// forwarded command (see [`daemon`]); `argv[0]` is the program name.
pub(crate) async fn run_argv(argv: Vec<String>) -> i32 {
    match Cli::try_parse_from(&argv) {
        Ok(cli) => run_and_report(cli).await,
        Err(e) => {
            use clap::error::ErrorKind;
            let rendered = e.render().to_string();
            match e.kind() {
                // --help / --version are "errors" that print to stdout, exit 0.
                ErrorKind::DisplayHelp
                | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                | ErrorKind::DisplayVersion => {
                    cli_print!("{rendered}");
                    0
                }
                _ => {
                    cli_eprint!("{rendered}");
                    2
                }
            }
        }
    }
}

/// Run a parsed command and print any error (JSON or plain), returning the exit code.
async fn run_and_report(cli: Cli) -> i32 {
    let json = cli.json;
    match run(cli).await {
        Ok(()) => 0,
        Err(err) => {
            if json {
                // Single line so it stays valid in the NDJSON streams (e.g. `install`).
                print_json_line(&serde_json::json!({ "error": format!("{err:#}") }));
            } else {
                cli_eprintln!("Error: {err:#}");
            }
            1
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    aurelia::config::ensure_config_dirs().await?;

    let json = cli.json;
    match cli.command {
        Command::Login {
            username,
            password,
            guard,
            qr,
            code,
            health,
            reconnect,
        } => cmd_login(username, password, guard, qr, code, health, reconnect, json).await,
        Command::Logout => cmd_logout(json).await,
        Command::List {
            installed,
            search,
            online,
        } => cmd_list(installed, search, online, json).await,
        Command::Account => cmd_account(json).await,
        Command::Install {
            app_id,
            platform,
            restart_steam,
            dry_run,
        } => cmd_install(app_id, platform, restart_steam, dry_run, json).await,
        Command::Uninstall {
            app_id,
            delete_prefix,
        } => cmd_uninstall(app_id, delete_prefix, json).await,
        Command::Move {
            app_id,
            library,
            restart_steam,
        } => cmd_move(app_id, library, restart_steam, json).await,
        Command::Relink {
            app_id,
            library,
            restart_steam,
        } => cmd_relink(app_id, library, restart_steam, json).await,
        Command::Import {
            app_id,
            library,
            platform,
            restart_steam,
        } => cmd_import(app_id, library, platform, restart_steam, json).await,
        Command::Available { app_id } => cmd_available(app_id, json).await,
        Command::Verify { app_id } => cmd_verify(app_id, json).await,
        Command::Update { app_id } => cmd_update(app_id, json).await,
        Command::Play {
            app_id,
            proton,
            windows,
        } => cmd_play(app_id, proton, windows, json).await,
        Command::Stop { app_id } => cmd_stop(app_id, json).await,
        Command::Enable {
            app_id,
            restart_steam,
        } => cmd_set_dlc(app_id, true, restart_steam, json).await,
        Command::Disable {
            app_id,
            restart_steam,
        } => cmd_set_dlc(app_id, false, restart_steam, json).await,
        Command::Branches { app_id } => cmd_branches(app_id, json).await,
        Command::SetBranch { app_id, branch } => cmd_set_branch(app_id, branch, json).await,
        Command::Info {
            app_ids,
            extended,
            no_cache,
        } => cmd_info(app_ids, extended, no_cache, json).await,
        Command::Dlc { app_id } => cmd_dlc(app_id, json).await,
        Command::Achievements { app_id, lang } => cmd_achievements(app_id, lang, json).await,
        Command::Depots { app_id } => cmd_depots(app_id, json).await,
        Command::LaunchOptions { app_id } => cmd_launch_options(app_id, json).await,
        Command::Image {
            app_id,
            output,
            force,
        } => cmd_image(app_id, output, force, json).await,
        Command::Config { command } => match command {
            ConfigCommand::Show => cmd_config_show(json).await,
            ConfigCommand::Protons => cmd_config_protons(json),
        },
        Command::Cloud { command } => match command {
            CloudCommand::Sync {
                app_id,
                up,
                down,
                path,
            } => cmd_cloud_sync(app_id, up, down, path, json).await,
            CloudCommand::List { app_id } => cmd_cloud_list(app_id, json).await,
        },
        Command::Workshop { command } => match command {
            WorkshopCommand::Browse {
                app_id,
                search,
                sort,
                count,
                cursor,
                tags,
            } => cmd_workshop_browse(app_id, search, sort, count, cursor, tags, json).await,
            WorkshopCommand::Info { ids } => cmd_workshop_info(ids, json).await,
            WorkshopCommand::List { app_id } => cmd_workshop_list(app_id, json).await,
            WorkshopCommand::Install { ids, no_recurse } => {
                cmd_workshop_install(ids, no_recurse, json).await
            }
            WorkshopCommand::Uninstall { ids, no_recurse } => {
                cmd_workshop_uninstall(ids, no_recurse, json).await
            }
            WorkshopCommand::Subscribe {
                ids,
                install,
                no_recurse,
            } => cmd_workshop_subscribe(ids, install, no_recurse, json).await,
            WorkshopCommand::Unsubscribe { ids, no_recurse } => {
                cmd_workshop_unsubscribe(ids, no_recurse, json).await
            }
            WorkshopCommand::Status { app_id } => cmd_workshop_status(app_id, json).await,
            WorkshopCommand::Rate { id, vote } => {
                cmd_workshop_rate(id, matches!(vote, VoteArg::Up), json).await
            }
            WorkshopCommand::Comments { id, count, start } => {
                cmd_workshop_comments_read(id, start, count, json).await
            }
            WorkshopCommand::Comment { id, text } => cmd_workshop_comment(id, text, json).await,
        },
        Command::Kill => cmd_kill(json),
        Command::Daemon {
            command: Some(sub), ..
        } => match sub {
            DaemonCommand::Stop { pid } => cmd_daemon_stop(pid, json),
            DaemonCommand::List => cmd_daemon_list(json),
        },
        // Bare `daemon` is intercepted in `async_main`; it never reaches here.
        Command::Daemon { command: None, .. } => {
            bail!("`aurelia daemon` cannot be run as a forwarded command")
        }
    }
}

/// Print a JSON value to stdout (pretty-printed).
fn print_json(value: &serde_json::Value) {
    match serde_json::to_string_pretty(value) {
        Ok(s) => cli_println!("{s}"),
        Err(_) => cli_println!("{{}}"),
    }
}

/// Print a JSON value as a single compact line (for NDJSON streams).
fn print_json_line(value: &serde_json::Value) {
    match serde_json::to_string(value) {
        Ok(s) => cli_println!("{s}"),
        Err(_) => cli_println!("{{}}"),
    }
}

/// Build a client and restore a persisted session if one exists.
///
/// Inside the daemon this returns a cheap client backed by the **single shared
/// connection** (no logon); only a standalone run actually re-authenticates here.
async fn restored_client() -> Result<SteamClient> {
    if daemon::in_daemon() {
        return Ok(daemon::shared_restored_client().await);
    }
    let mut client = SteamClient::new()?;
    let saved = load_session().await.unwrap_or_default();
    if saved.refresh_token.is_some() && saved.account_name.is_some() {
        tracing::info!("Restoring Steam session (connecting to Steam) ...");
        match client.restore_session().await {
            Ok(_) => tracing::info!("Restored Steam session from refresh token"),
            Err(e) => tracing::warn!("Stored refresh token failed ({e:#}); run `aurelia login`"),
        }
    }
    Ok(client)
}

/// Require an authenticated client, erroring out with a helpful message otherwise.
async fn authed_client() -> Result<SteamClient> {
    let client = restored_client().await?;
    if !client.is_authenticated() {
        bail!("not logged in — run `aurelia login` first");
    }
    Ok(client)
}

/// Build the merged owned + installed library.
async fn load_library(client: &mut SteamClient) -> Vec<LibraryGame> {
    let cached = load_library_cache().await.unwrap_or_default();
    let owned = if client.is_authenticated() {
        tracing::info!("Fetching owned games from Steam ...");
        match client.fetch_owned_games().await {
            Ok(games) => {
                tracing::info!("Fetched {} owned games", games.len());
                games
            }
            Err(e) => {
                tracing::warn!("Could not fetch owned games ({e:#}); using cached library");
                cached
            }
        }
    } else if !cached.is_empty() {
        cached
    } else {
        // Not logged in to Aurelia (and nothing cached). The Steam client is
        // almost always already signed in on Linux and keeps the whole library
        // on disk, so fall back to reading its caches. This makes `list` show
        // the full library instead of only locally-installed games.
        aurelia::local_library::discover_local_owned_games().await
    };
    let installed = scan_installed_app_info().await.unwrap_or_default();
    build_game_library(owned, installed, client.steam_id()).games
}

/// Merge Family-Shared apps into the library. Apps already present (e.g. installed,
/// or surfaced via another path) are flagged as family-shared if not owned; apps not
/// yet present are added as non-installed family-shared entries.
fn merge_family_shared(games: &mut Vec<LibraryGame>, shared: Vec<SharedApp>) {
    for app in shared {
        if aurelia::library::is_ignored_steam_app(app.app_id, &app.name) {
            continue;
        }
        if let Some(existing) = games.iter_mut().find(|g| g.app_id == app.app_id) {
            if !existing.is_owned {
                existing.is_family_shared = true;
            }
            continue;
        }
        games.push(LibraryGame {
            app_id: app.app_id,
            name: app.name,
            playtime_forever_minutes: None,
            is_installed: false,
            install_path: None,
            local_manifest_ids: Default::default(),
            update_available: false,
            update_queued: false,
            active_branch: "public".to_string(),
            is_owned: false,
            is_family_shared: true,
            online_required: None,
        });
    }
}

async fn find_game(client: &mut SteamClient, app_id: u32) -> Result<LibraryGame> {
    load_library(client)
        .await
        .into_iter()
        .find(|g| g.app_id == app_id)
        .with_context(|| format!("app {app_id} is not in your library"))
}

#[allow(clippy::too_many_arguments)]
async fn cmd_login(
    username: Option<String>,
    password: Option<String>,
    guard: Option<String>,
    qr: bool,
    code: bool,
    health: bool,
    reconnect: bool,
    json: bool,
) -> Result<()> {
    if health {
        return cmd_login_health(json).await;
    }
    if reconnect {
        return cmd_login_reconnect(json).await;
    }
    if qr {
        return cmd_login_qr(json).await;
    }

    // In `--json` mode the login is driven non-interactively (e.g. by Heroic): no
    // TTY prompts. Credentials come from flags/env, and a Steam Guard code is
    // requested via a `{event:"guard_required",...}` line and read back from stdin.
    if json {
        return cmd_login_json(username, password, guard).await;
    }

    let username = match username {
        Some(u) => u,
        None => prompt_line("Steam username: ")?,
    };
    let account = username.clone();
    let password = match password.or_else(|| std::env::var("AURELIA_PASSWORD").ok()) {
        Some(p) => p,
        None => rpassword::prompt_password("Steam password: ")
            .context("failed reading password")?,
    };

    let mut client = SteamClient::new()?;
    // `--code` (alias `--pin`) reads the Steam Guard code interactively from stdin
    // (handled inside `login`); otherwise we wait for mobile-app approval and only
    // prompt for a code on the retry path below.
    let attempt = client
        .login(username.clone(), password.clone(), guard.clone(), code)
        .await;

    match attempt {
        Ok(_) => {
            report_login_success(&account, json);
            Ok(())
        }
        Err(err) => {
            // If Steam asked for a Guard code and we don't have one yet, prompt and retry once.
            let needs_code = guard.is_none()
                && client.pending_confirmations().iter().any(|p| {
                    use aurelia::models::SteamGuardReq::{DeviceCode, EmailCode};
                    matches!(p.requirement, EmailCode { .. } | DeviceCode)
                });

            if needs_code {
                tracing::info!("Login method awaited: Steam Guard code");
                let code = prompt_line("Steam Guard code: ")?;
                client
                    .login(username, password, Some(code), false)
                    .await
                    .context("login failed after providing Steam Guard code")?;
                report_login_success(&account, json);
                Ok(())
            } else if client
                .pending_confirmations()
                .iter()
                .any(|p| matches!(p.requirement, aurelia::models::SteamGuardReq::DeviceConfirmation))
            {
                tracing::info!("Login method awaited: Steam Mobile app approval");
                bail!("approve this login in the Steam Mobile app, then run `aurelia login` again")
            } else {
                Err(err).context("login failed")
            }
        }
    }
}

/// Log in by scanning a QR code with the Steam Mobile app.
///
/// In `--json` mode the challenge URL is streamed as `{event:"qr_challenge",url}`
/// (re-emitted whenever Steam rotates the code) so a driver like Heroic can render
/// the QR itself; otherwise it's drawn to stderr as a terminal QR.
async fn cmd_login_qr(json: bool) -> Result<()> {
    let mut client = SteamClient::new()?;
    let result = if json {
        client.login_qr(emit_qr_challenge_json).await
    } else {
        client.login_qr(render_login_qr).await
    };
    let session = result.context("QR login failed")?;
    let account = session.account_name.clone().unwrap_or_default();
    report_login_success(&account, json);
    Ok(())
}

/// How long to wait for a single Steam login attempt before giving up. The login
/// call blocks inside steam-vent while it waits for a Steam Guard code or for the
/// user to approve the login in the Steam Mobile app; this bounds that wait so a
/// `--json` driver never hangs indefinitely.
const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Run one `login` attempt with [`LOGIN_TIMEOUT`]. On timeout, returns a clear
/// error (so a `--json` driver gets `{ "error": ... }` rather than hanging).
async fn login_with_timeout(
    client: &mut SteamClient,
    username: &str,
    password: &str,
    guard: Option<String>,
) -> Result<aurelia::models::SessionState> {
    match tokio::time::timeout(
        LOGIN_TIMEOUT,
        client.login(username.to_string(), password.to_string(), guard, false),
    )
    .await
    {
        Ok(login_result) => login_result,
        Err(_) => bail!(
            "login timed out after {}s waiting for a Steam Guard code or Steam Mobile app approval",
            LOGIN_TIMEOUT.as_secs()
        ),
    }
}

/// Non-interactive password login for `--json` drivers (e.g. Heroic). Credentials
/// come from flags or `AURELIA_PASSWORD`. To keep the driver informed (and never
/// silent), it emits:
/// - `{event:"awaiting_confirmation"}` right away — the login may block while
///   Steam waits for the Guard code or Mobile-app approval, so the driver should
///   prompt the user to approve on their device;
/// - `{event:"guard_required",type:"email"|"device"}` if a typed Guard code is
///   needed and none was supplied (the code is then read as one line from stdin);
/// - `{event:"guard_required",type:"device_confirmation"}` for mobile-approval
///   accounts;
/// - finally `{logged_in:true,...}` or `{error:...}`.
///
/// Each login attempt is bounded by [`LOGIN_TIMEOUT`].
async fn cmd_login_json(
    username: Option<String>,
    password: Option<String>,
    guard: Option<String>,
) -> Result<()> {
    let username =
        username.context("--json login requires a username (-u/--username)")?;
    let password = password
        .or_else(|| std::env::var("AURELIA_PASSWORD").ok())
        .context("--json login requires a password (-p/--password or AURELIA_PASSWORD)")?;
    let account = username.clone();

    let mut client = SteamClient::new()?;

    // The login call below blocks inside steam-vent while it waits for the Guard
    // code / mobile confirmation. Emit this first so the driver can immediately
    // tell the user to approve the login (otherwise it sees no output until the
    // attempt completes or times out).
    print_json_line(&serde_json::json!({
        "event": "awaiting_confirmation",
        "message": "Signing in — if prompted, approve this login in your Steam Mobile app."
    }));

    match login_with_timeout(&mut client, &username, &password, guard.clone()).await {
        Ok(_) => {
            report_login_success(&account, true);
            Ok(())
        }
        Err(err) => {
            use aurelia::models::SteamGuardReq::{DeviceCode, DeviceConfirmation, EmailCode};

            // A typed Steam Guard code is needed (email or authenticator).
            let code_kind = guard.is_none().then(|| {
                client.pending_confirmations().iter().find_map(|p| match p.requirement {
                    EmailCode { .. } => Some("email"),
                    DeviceCode => Some("device"),
                    _ => None,
                })
            }).flatten();

            if let Some(kind) = code_kind {
                print_json_line(&serde_json::json!({ "event": "guard_required", "type": kind }));
                let code = read_stdin_line()
                    .await
                    .context("failed reading Steam Guard code from stdin")?;
                login_with_timeout(&mut client, &username, &password, Some(code))
                    .await
                    .context("login failed after providing Steam Guard code")?;
                report_login_success(&account, true);
                Ok(())
            } else if client
                .pending_confirmations()
                .iter()
                .any(|p| matches!(p.requirement, DeviceConfirmation))
            {
                print_json_line(
                    &serde_json::json!({ "event": "guard_required", "type": "device_confirmation" }),
                );
                bail!("approve this login in the Steam Mobile app, then run login again")
            } else {
                Err(err).context("login failed")
            }
        }
    }
}

/// Emit a QR login challenge URL as one NDJSON line for a `--json` driver.
fn emit_qr_challenge_json(url: &str) {
    print_json_line(&serde_json::json!({ "event": "qr_challenge", "url": url }));
}

/// Read a single line from stdin (used to receive a Guard code from a `--json`
/// driver). Returns the trimmed contents. Routes through [`output::read_line`] so
/// that, inside the daemon, it reads the forwarding client's stdin.
async fn read_stdin_line() -> Result<String> {
    output::read_line()
        .await
        .context("failed reading stdin")
        .map(|s| s.trim().to_string())
}

/// Render a Steam login challenge URL as a scannable QR code on stderr, with the
/// raw URL as a fallback. Diagnostics go to stderr so stdout stays clean.
fn render_login_qr(url: &str) {
    match qrcode::QrCode::new(url.as_bytes()) {
        Ok(code) => {
            let rendered = code
                .render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .build();
            cli_eprintln!("\nScan this QR code with the Steam Mobile app:\n{rendered}");
        }
        Err(e) => cli_eprintln!("\n(could not render QR code: {e})"),
    }
    cli_eprintln!("Or open this link in the Steam Mobile app:\n  {url}\n");
}

fn report_login_success(account: &str, json: bool) {
    if json {
        print_json(&serde_json::json!({ "logged_in": true, "account": account }));
    } else {
        cli_println!("Login successful.");
    }
}

async fn cmd_logout(json: bool) -> Result<()> {
    let mut client = restored_client().await?;
    client.logout().await?;
    if json {
        print_json(&serde_json::json!({ "logged_out": true }));
    } else {
        cli_println!("Logged out.");
    }
    Ok(())
}

/// `login --health`: report whether a session is authenticated, without logging in.
/// When a daemon is in use this reflects its shared session (no new logon); standalone
/// it does a one-off live restore check.
async fn cmd_login_health(json: bool) -> Result<()> {
    let via_daemon = daemon::in_daemon();
    let status = if via_daemon {
        daemon::session_status().await
    } else {
        let client = restored_client().await?;
        daemon::SessionStatus {
            authenticated: client.is_authenticated(),
            account: load_session().await.ok().and_then(|s| s.account_name),
            steam_id: client.steam_id(),
        }
    };
    report_session_status(&status, via_daemon, json);
    Ok(())
}

/// `login --reconnect`: tear down and re-establish the daemon's shared session from
/// the stored token (for use after the live connection dropped).
async fn cmd_login_reconnect(json: bool) -> Result<()> {
    if !daemon::in_daemon() {
        bail!(
            "--reconnect needs the session daemon, but this command is running standalone \
             (AURELIA_NO_DAEMON is set, or the daemon is unreachable). Start `aurelia daemon` first."
        );
    }
    let status = daemon::force_reconnect().await;
    report_session_status(&status, true, json);
    Ok(())
}

/// Print a [`daemon::SessionStatus`] for `--health`/`--reconnect`.
fn report_session_status(status: &daemon::SessionStatus, via_daemon: bool, json: bool) {
    if json {
        print_json(&serde_json::json!({
            "logged_in": status.authenticated,
            "account": status.account,
            "steam_id": status.steam_id,
            "daemon": via_daemon,
        }));
    } else {
        cli_println!(
            "Session : {}",
            if status.authenticated { "authenticated" } else { "not logged in" }
        );
        if let Some(account) = &status.account {
            cli_println!("Account : {account}");
        }
        if let Some(steam_id) = status.steam_id {
            cli_println!("SteamID : {steam_id}");
        }
        cli_println!(
            "Daemon  : {}",
            if via_daemon { "yes (shared session)" } else { "no (standalone)" }
        );
    }
}

async fn cmd_list(
    installed: bool,
    search: Option<String>,
    online: bool,
    json: bool,
) -> Result<()> {
    let mut client = restored_client().await?;
    let mut games = load_library(&mut client).await;

    // Include Family-Shared games that aren't installed (and which we don't own),
    // such as titles only available through a family member's library.
    if client.is_authenticated() {
        tracing::info!("Fetching Family Sharing library from Steam ...");
        match client.fetch_family_shared_apps().await {
            Ok(shared) => {
                tracing::info!("Fetched {} Family-Shared apps", shared.len());
                merge_family_shared(&mut games, shared);
            }
            Err(e) => tracing::warn!("could not fetch family shared apps: {e:#}"),
        }
    }

    if installed {
        games.retain(|g| g.is_installed);
    }
    if let Some(needle) = search.as_deref().map(str::to_ascii_lowercase) {
        games.retain(|g| g.name.to_ascii_lowercase().contains(&needle));
    }
    games.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));

    // Resolve the "online required" status only when requested: it needs a PICS
    // appinfo fetch per game, which is too costly to do on every `list`.
    if online {
        if client.is_authenticated() && !client.is_offline() {
            tracing::info!("Resolving online-required status for {} game(s) ...", games.len());
            for g in &mut games {
                match client.fetch_online_required(g.app_id).await {
                    Ok(required) => g.online_required = Some(required),
                    Err(e) => {
                        tracing::warn!("could not determine online-required for {}: {e:#}", g.app_id);
                    }
                }
            }
        } else {
            tracing::warn!(
                "--online needs an authenticated, online session; ONLINE column will be unknown"
            );
        }
    }

    if json {
        cli_println!("{}", serde_json::to_string_pretty(&games)?);
        return Ok(());
    }

    if games.is_empty() {
        cli_println!("No games match.");
        return Ok(());
    }

    if online {
        cli_println!(
            "{:>9}  {:<10}  {:<13}  {:<7}  NAME",
            "APPID", "STATUS", "LICENSE", "ONLINE"
        );
    } else {
        cli_println!("{:>9}  {:<10}  {:<13}  NAME", "APPID", "STATUS", "LICENSE");
    }
    for g in &games {
        let status = if g.is_installed {
            if g.update_available {
                "update"
            } else {
                "installed"
            }
        } else {
            "-"
        };
        let license = if g.is_owned {
            "owned"
        } else if g.is_family_shared {
            "family-shared"
        } else {
            "unlicensed"
        };
        let branch = if g.active_branch != "public" {
            format!(" [{}]", g.active_branch)
        } else {
            String::new()
        };
        if online {
            let online_col = match g.online_required {
                Some(true) => "yes",
                Some(false) => "no",
                None => "?",
            };
            cli_println!(
                "{:>9}  {:<10}  {:<13}  {:<7}  {}{}",
                g.app_id, status, license, online_col, g.name, branch
            );
        } else {
            cli_println!(
                "{:>9}  {:<10}  {:<13}  {}{}",
                g.app_id, status, license, g.name, branch
            );
        }
    }

    let shared = games.iter().filter(|g| g.is_family_shared).count();
    if shared > 0 {
        cli_println!(
            "\n{} game(s), {} via Family Sharing (not licensed to this account).",
            games.len(),
            shared
        );
    } else {
        cli_println!("\n{} game(s).", games.len());
    }
    Ok(())
}

async fn cmd_account(json: bool) -> Result<()> {
    let client = authed_client().await?;
    let data = client.get_account_data().await;

    if json {
        let value = serde_json::json!({
            "steam_id": data.steam_id,
            "account_name": data.account_name,
            "country": data.country,
            "email": data.email,
            "email_validated": data.email_validated,
            "authed_machines": data.authed_machines,
            "flags": data.flags,
            "vac_bans": data.vac_bans,
            "vac_banned_apps": data.vac_banned_apps,
        });
        cli_println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    cli_println!("Account : {}", data.account_name);
    cli_println!("SteamID : {}", data.steam_id);
    cli_println!("Country : {}", data.country);
    cli_println!(
        "Email   : {} ({})",
        data.email,
        if data.email_validated {
            "validated"
        } else {
            "unvalidated"
        }
    );
    cli_println!("Devices : {}", data.authed_machines);
    cli_println!("VAC bans: {}", data.vac_bans);
    Ok(())
}

async fn cmd_install(
    app_id: u32,
    platform: Option<PlatformArg>,
    restart_steam: bool,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    let mut client = authed_client().await?;

    // `--dry-run` reports the size estimate and stops — it never touches Steam or
    // downloads anything.
    if dry_run {
        return cmd_install_dry_run(&mut client, app_id, platform, json).await;
    }

    // Note whether this is a DLC so we can refresh Steam's view afterward.
    let is_dlc = client.resolve_dlc_parent(app_id).await.is_some();

    // For a DLC, stop Steam before editing its base appmanifest (Steam overwrites it
    // on exit), then restart it afterward so the running client picks up the change.
    let manage_steam = restart_steam && is_dlc && SteamClient::steam_is_running();
    if manage_steam {
        if !json {
            cli_println!("Stopping Steam ...");
        }
        SteamClient::shutdown_steam()?;
    }

    let (platform, cached_vdf) = match platform {
        Some(p) => (p.into(), None),
        None => {
            let (platforms, buffer) = client
                .get_available_platforms(app_id)
                .await
                .context("failed to detect available platforms")?;
            let chosen = platforms
                .first()
                .copied()
                .unwrap_or(DepotPlatform::Windows);
            if !json {
                cli_println!("Auto-selected platform: {chosen:?}");
            }
            (chosen, Some(buffer))
        }
    };

    let state = Arc::new(RwLock::new(DownloadState::default()));
    let rx = client
        .install_game(app_id, platform, cached_vdf, None, state)
        .await
        .with_context(|| format!("failed to start install for app {app_id}"))?;
    drive_progress(rx, json).await?;

    let mut steam_restarted = false;
    if manage_steam {
        if !json {
            cli_println!("Starting Steam ...");
        }
        SteamClient::start_steam()?;
        steam_restarted = true;
    }

    // A newly installed DLC is invisible to an already-running Steam client until it
    // re-reads the appmanifest (which it does at startup).
    let steam_restart_required = is_dlc && !manage_steam && SteamClient::steam_is_running();
    if steam_restart_required && !json {
        cli_eprintln!();
        cli_eprintln!("Note: the DLC content is installed, but a running Steam client reads DLC state");
        cli_eprintln!("      only at startup. Restart Steam (or re-run with --restart-steam) for it to");
        cli_eprintln!("      be recognized in-game.");
    }

    if json {
        print_json_line(&serde_json::json!({
            "event": "result",
            "app_id": app_id,
            "status": "installed",
            "dlc": is_dlc,
            "steam_restart_required": steam_restart_required,
            "steam_restarted": steam_restarted,
        }));
    }
    Ok(())
}

/// Report the estimated download/disk size for installing `app_id` without
/// installing anything (mirrors Nile's `install --info --json`).
async fn cmd_install_dry_run(
    client: &mut SteamClient,
    app_id: u32,
    platform: Option<PlatformArg>,
    json: bool,
) -> Result<()> {
    let platform: DepotPlatform = match platform {
        Some(p) => p.into(),
        None => {
            let (platforms, _) = client
                .get_available_platforms(app_id)
                .await
                .context("failed to detect available platforms")?;
            platforms.first().copied().unwrap_or(DepotPlatform::Windows)
        }
    };

    let est = client
        .estimate_install_size(app_id, platform)
        .await
        .with_context(|| format!("failed to estimate install size for app {app_id}"))?;

    let platform_str = format!("{platform:?}").to_lowercase();
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "platform": platform_str,
            "download_size": est.download_size,
            "disk_size": est.disk_size,
            "depot_count": est.depot_count,
        }));
    } else {
        cli_println!("Install estimate for app {app_id} ({platform_str}):");
        cli_println!("  Download size: {}", human_bytes(est.download_size));
        cli_println!("  Disk size    : {}", human_bytes(est.disk_size));
        cli_println!("  Depots       : {}", est.depot_count);
    }
    Ok(())
}

/// Format a byte count as a human-readable size (binary units).
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

async fn cmd_uninstall(app_id: u32, delete_prefix: bool, json: bool) -> Result<()> {
    let client = authed_client().await?;
    client
        .uninstall_game(app_id, delete_prefix)
        .await
        .with_context(|| format!("failed to uninstall app {app_id}"))?;
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "uninstalled": true,
            "deleted_prefix": delete_prefix,
        }));
    } else {
        cli_println!("Uninstalled app {app_id}.");
    }
    Ok(())
}

async fn cmd_move(
    app_id: u32,
    library: PathBuf,
    restart_steam: bool,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;

    // Steam rewrites appmanifests and libraryfolders.vdf on exit, so it must not be
    // running while we move things.
    let managed = steam_guard_stop(restart_steam, json)?;
    let rx = client
        .move_install(app_id, library.clone())
        .await
        .with_context(|| format!("failed to start moving app {app_id}"))?;
    let outcome = drive_progress(rx, json).await;
    steam_guard_restart(managed, json)?;
    outcome.with_context(|| format!("failed to move app {app_id}"))?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "status": "moved",
            "library": library.to_string_lossy(),
            "steam_restarted": managed,
        }));
    } else {
        cli_println!("Moved app {app_id} to {}.", library.display());
    }
    Ok(())
}

/// Steam edits to appmanifests / `libraryfolders.vdf` are clobbered if Steam is
/// running (it rewrites them on exit). If Steam is up, either stop it (when
/// `restart_steam`) — returning `true` so the caller restarts it afterward — or
/// refuse. Returns whether Steam was stopped.
fn steam_guard_stop(restart_steam: bool, json: bool) -> Result<bool> {
    let running = SteamClient::steam_is_running();
    if running && !restart_steam {
        bail!(
            "Steam is running. Close it first, or re-run with --restart-steam to have \
             Aurelia stop and restart it around the change."
        );
    }
    if running {
        if !json {
            cli_println!("Stopping Steam ...");
        }
        SteamClient::shutdown_steam()?;
        return Ok(true);
    }
    Ok(false)
}

/// Restart Steam if [`steam_guard_stop`] stopped it.
fn steam_guard_restart(managed: bool, json: bool) -> Result<()> {
    if managed {
        if !json {
            cli_println!("Starting Steam ...");
        }
        SteamClient::start_steam()?;
    }
    Ok(())
}

async fn cmd_relink(
    app_id: u32,
    library: PathBuf,
    restart_steam: bool,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;
    let managed = steam_guard_stop(restart_steam, json)?;
    let result = client.relink_install(app_id, library.clone()).await;
    steam_guard_restart(managed, json)?;
    let path = result.with_context(|| format!("failed to relink app {app_id}"))?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "status": "relinked",
            "library": library.to_string_lossy(),
            "install_path": path.to_string_lossy(),
            "steam_restarted": managed,
        }));
    } else {
        cli_println!("Relinked app {app_id} to {}.", library.display());
    }
    Ok(())
}

async fn cmd_import(
    app_id: u32,
    library: PathBuf,
    platform: Option<PlatformArg>,
    restart_steam: bool,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;
    // Default to the OS we're running on for the depot/platform match.
    let platform: DepotPlatform = platform.map(Into::into).unwrap_or(if cfg!(target_os = "windows") {
        DepotPlatform::Windows
    } else {
        DepotPlatform::Linux
    });

    let managed = steam_guard_stop(restart_steam, json)?;
    let result = client.import_install(app_id, library.clone(), platform).await;
    steam_guard_restart(managed, json)?;
    let path = result.with_context(|| format!("failed to import app {app_id}"))?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "status": "imported",
            "library": library.to_string_lossy(),
            "install_path": path.to_string_lossy(),
            "steam_restarted": managed,
        }));
    } else {
        cli_println!("Imported app {app_id} from {}.", path.display());
    }
    Ok(())
}

async fn cmd_available(app_id: u32, json: bool) -> Result<()> {
    // `is_game_available` only reads the local appmanifest and checks the files on
    // disk, so we deliberately build a client *without* restoring the Steam session.
    // A driver like Heroic calls `available` per game on every refresh; restoring
    // the session here would mean one Steam CM logon per call (and Steam throttles
    // repeated logons hard) for data we never fetch over the wire.
    let client = SteamClient::new()?;
    let (available, install_path) = client.is_game_available(app_id).await;
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "available": available,
            "install_path": install_path,
        }));
    } else {
        cli_println!(
            "App {app_id}: {}",
            if available { "available" } else { "not available" }
        );
        if let Some(p) = install_path {
            cli_println!("  path: {p}");
        }
    }
    Ok(())
}

async fn cmd_verify(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let state = Arc::new(RwLock::new(DownloadState::default()));
    let rx = client
        .verify_game(app_id, state)
        .await
        .with_context(|| format!("failed to verify app {app_id}"))?;
    drive_progress(rx, json).await?;
    report_operation(app_id, "verified", json);
    Ok(())
}

async fn cmd_update(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let state = Arc::new(RwLock::new(DownloadState::default()));
    let rx = client
        .update_game(app_id, state)
        .await
        .with_context(|| format!("failed to update app {app_id}"))?;
    drive_progress(rx, json).await?;
    report_operation(app_id, "updated", json);
    Ok(())
}

/// Print the final result of a streaming operation (install/verify/update).
fn report_operation(app_id: u32, status: &str, json: bool) {
    if json {
        print_json(&serde_json::json!({ "app_id": app_id, "status": status }));
    }
}

async fn cmd_play(app_id: u32, proton: Option<String>, windows: bool, json: bool) -> Result<()> {
    let mut client = authed_client().await?;
    let game = find_game(&mut client, app_id).await?;

    // Proton/Wine is Linux-only; on Windows we always run the game natively.
    let force_windows = windows || cfg!(target_os = "windows");

    let launcher_config = load_launcher_config().await.unwrap_or_default();
    let prefers_windows = launcher_config
        .game_configs
        .get(&app_id)
        .and_then(|c| c.platform_preference.as_deref())
        == Some("windows");

    let proton_path = if let Some(p) = proton {
        Some(p)
    } else if prefers_windows {
        Some(launcher_config.proton_version.clone())
    } else {
        None
    };

    let user_configs = load_user_configs().await.unwrap_or_default();
    let user_config = user_configs.get(&app_id);

    if !json {
        cli_println!("Launching {} ...", game.name);
    }
    client
        .play_game(&game, proton_path.as_deref(), user_config, force_windows)
        .await
        .with_context(|| format!("failed to launch {}", game.name))?;
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "name": game.name,
            "status": "finished",
        }));
    } else {
        cli_println!("Finished playing {}.", game.name);
    }
    Ok(())
}

async fn cmd_stop(app_id: Option<u32>, json: bool) -> Result<()> {
    // No app id: report what Aurelia currently tracks as running.
    let Some(app_id) = app_id else {
        let running = aurelia::running::list();
        if json {
            let arr: Vec<_> = running
                .iter()
                .map(|g| serde_json::json!({ "app_id": g.app_id, "name": g.name, "pid": g.pid }))
                .collect();
            print_json(&serde_json::json!({ "running": arr }));
            return Ok(());
        }
        if running.is_empty() {
            cli_println!("No games are running (none launched via `aurelia play`).");
        } else {
            cli_println!("{:>9}  {:>8}  NAME", "APPID", "PID");
            for g in &running {
                cli_println!("{:>9}  {:>8}  {}", g.app_id, g.pid, g.name);
            }
        }
        return Ok(());
    };

    let stopped = SteamClient::stop_game(app_id)
        .with_context(|| format!("failed to stop app {app_id}"))?;
    if json {
        print_json(&serde_json::json!({
            "app_id": stopped.app_id,
            "name": stopped.name,
            "status": "stopped",
        }));
    } else {
        cli_println!("Stopped {} (app {}).", stopped.name, stopped.app_id);
    }
    Ok(())
}

/// `aurelia kill`: terminate every running aurelia process (daemon and otherwise),
/// except the current one.
fn cmd_kill(json: bool) -> Result<()> {
    let procs = proc_admin::find_aurelia_processes();
    let pids: Vec<u32> = procs.iter().map(|p| p.pid).collect();
    let killed = proc_admin::kill_pids(&pids);

    if json {
        print_json(&serde_json::json!({ "found": pids.len(), "killed": killed, "pids": pids }));
    } else if pids.is_empty() {
        cli_println!("No other aurelia processes are running.");
    } else {
        cli_println!("Killed {killed} of {} aurelia process(es) (including the daemon).", pids.len());
    }
    Ok(())
}

/// `aurelia daemon stop [PID]`: terminate the session daemon(s). With a PID, stop
/// only that daemon (erroring if it isn't a running aurelia daemon).
fn cmd_daemon_stop(pid: Option<u32>, json: bool) -> Result<()> {
    let daemons: Vec<_> = proc_admin::find_aurelia_processes()
        .into_iter()
        .filter(|p| p.is_daemon)
        .collect();

    let targets: Vec<u32> = match pid {
        Some(pid) => {
            if !daemons.iter().any(|d| d.pid == pid) {
                bail!("PID {pid} is not a running aurelia daemon (see `aurelia daemon list`)");
            }
            vec![pid]
        }
        None => daemons.iter().map(|d| d.pid).collect(),
    };
    let killed = proc_admin::kill_pids(&targets);

    if json {
        print_json(&serde_json::json!({ "killed": killed, "pids": targets }));
    } else if targets.is_empty() {
        cli_println!("No aurelia daemon is running.");
    } else {
        cli_println!("Stopped {killed} aurelia daemon(s).");
    }
    Ok(())
}

/// `aurelia daemon list`: show running aurelia daemon(s) and their PIDs.
fn cmd_daemon_list(json: bool) -> Result<()> {
    let daemons: Vec<_> = proc_admin::find_aurelia_processes()
        .into_iter()
        .filter(|p| p.is_daemon)
        .collect();

    if json {
        let arr: Vec<_> = daemons
            .iter()
            .map(|d| serde_json::json!({ "pid": d.pid, "command": d.command }))
            .collect();
        print_json(&serde_json::json!({ "daemons": arr }));
        return Ok(());
    }

    if daemons.is_empty() {
        cli_println!("No aurelia daemon is running.");
        return Ok(());
    }
    cli_println!("{:>8}  COMMAND", "PID");
    for d in &daemons {
        cli_println!("{:>8}  {}", d.pid, d.command);
    }
    Ok(())
}

async fn cmd_set_dlc(app_id: u32, enable: bool, restart_steam: bool, json: bool) -> Result<()> {
    let client = restored_client().await?;

    // Steam flushes its in-memory app state on exit, so the edit must happen while
    // Steam is stopped to survive. With --restart-steam: stop → edit → start.
    let manage_steam = restart_steam && SteamClient::steam_is_running();
    if manage_steam {
        if !json {
            cli_println!("Stopping Steam ...");
        }
        SteamClient::shutdown_steam()?;
    }

    let base = client
        .set_dlc_enabled(app_id, enable)
        .await
        .with_context(|| {
            format!(
                "failed to {} DLC {app_id}",
                if enable { "enable" } else { "disable" }
            )
        })?;

    let mut steam_restarted = false;
    if manage_steam {
        if !json {
            cli_println!("Starting Steam ...");
        }
        SteamClient::start_steam()?;
        steam_restarted = true;
    }

    let action = if enable { "enabled" } else { "disabled" };
    let restart_required = !manage_steam && SteamClient::steam_is_running();

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "base_app_id": base,
            "status": action,
            "steam_restarted": steam_restarted,
            "steam_restart_required": restart_required,
        }));
        return Ok(());
    }

    cli_println!("DLC {app_id} {action} for base game {base}.");
    if enable {
        cli_println!("(Toggles the flag only — run `aurelia install {app_id}` if the content isn't downloaded.)");
    }
    if restart_required {
        cli_eprintln!("Note: Steam is running and reads DLC state only at startup, so this won't apply");
        cli_eprintln!("      until you restart Steam (or re-run with --restart-steam).");
    }
    Ok(())
}

async fn cmd_branches(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let branches = client
        .fetch_branches(app_id)
        .await
        .with_context(|| format!("failed to fetch branches for app {app_id}"))?;
    if json {
        print_json(&serde_json::json!({ "app_id": app_id, "branches": branches }));
        return Ok(());
    }
    if branches.is_empty() {
        cli_println!("No branches reported.");
    } else {
        for b in branches {
            cli_println!("{b}");
        }
    }
    Ok(())
}

async fn cmd_set_branch(app_id: u32, branch: String, json: bool) -> Result<()> {
    let client = authed_client().await?;
    client
        .update_app_branch(app_id, &branch)
        .await
        .with_context(|| format!("failed to switch app {app_id} to branch {branch}"))?;
    if json {
        print_json(&serde_json::json!({ "app_id": app_id, "branch": branch, "status": "set" }));
    } else {
        cli_println!("App {app_id} set to branch '{branch}'. Run `aurelia update {app_id}` to apply.");
    }
    Ok(())
}

/// Storefront-only `--extended` data for one app: the HTTPS `AppDetails` plus the
/// SteamSpy user tags.
type ExtendedInfo = (aurelia::store::AppDetails, Vec<String>);

async fn cmd_info(app_ids: Vec<u32>, extended: bool, no_cache: bool, json: bool) -> Result<()> {
    // The CM-sourced metadata (StoreBrowse + the DLC list) is effectively static
    // for hours, and drivers like Heroic call `info` repeatedly. Serve it from a
    // short-TTL disk cache so a repeat call avoids the Steam CM logon and the
    // StoreBrowse/PICS round-trips entirely. `--no-cache` forces a fresh fetch.
    let ttl = if no_cache {
        std::time::Duration::ZERO
    } else {
        info_cache_ttl()
    };
    let single = app_ids.len() == 1;

    // Partition the requested ids into cache hits and misses. Every miss is then
    // fetched over a *single* Steam logon with one batched StoreBrowse call, so
    // `info 1 2 3` costs one logon — not the three a caller spends running `info 1`,
    // `info 2`, `info 3` separately.
    let mut base: std::collections::HashMap<u32, (StoreAppInfo, Vec<(u32, Option<String>)>)> =
        std::collections::HashMap::new();
    let mut misses: Vec<u32> = Vec::new();
    for &id in &app_ids {
        match load_info_cache(id, ttl).await {
            Some(cached) => {
                base.insert(id, (cached.details, cached.dlc));
            }
            None => misses.push(id),
        }
    }

    if !misses.is_empty() {
        // Metadata comes from the StoreBrowse service over the Steam CM connection
        // (no HTTPS storefront API), so a session is needed here.
        let client = authed_client().await?;
        let store = client
            .fetch_store_apps(&misses)
            .await
            .context("failed to fetch store information")?;
        for &id in &misses {
            let Some(details) = store.iter().find(|i| i.app_id == id).cloned() else {
                // A single-id caller expects a hard error for an unknown app; in a
                // batch we skip it (with a warning) so one delisted id doesn't sink
                // the rest.
                if single {
                    bail!("no store information available for app {id}");
                }
                tracing::warn!("no store information available for app {id}; skipping");
                continue;
            };

            // The DLC id list isn't part of StoreBrowse's per-item data; read it
            // from PICS appinfo (`common.dlc`), then resolve the DLC names in a
            // single batched StoreBrowse call.
            let dlc_ids = client
                .get_extended_app_info(id)
                .await
                .map(|e| e.dlcs)
                .unwrap_or_default();
            let dlc = resolve_dlc_names_via_store(&client, &dlc_ids).await;

            // Best-effort cache write — a failure here must not fail the command.
            if let Err(e) = save_info_cache(id, &details, &dlc).await {
                tracing::warn!("could not cache info for app {id}: {e:#}");
            }
            base.insert(id, (details, dlc));
        }
    }

    // Storefront-only fields (system requirements, Metacritic, website, store
    // genres/categories, SteamSpy user tags). These have no CM-protocol source, so
    // `--extended` fetches them from the public HTTPS storefront, reusing one HTTP
    // client across ids. Best-effort: any failure leaves them absent.
    let mut extended_by_id: std::collections::HashMap<u32, ExtendedInfo> =
        std::collections::HashMap::new();
    if extended {
        match reqwest::Client::builder().user_agent("aurelia/0.1").build() {
            Ok(http) => {
                for &id in &app_ids {
                    if !base.contains_key(&id) {
                        continue;
                    }
                    let web = aurelia::store::fetch_app_details(&http, id).await.ok().flatten();
                    let tags = aurelia::store::fetch_tags(&http, id).await;
                    if let Some(d) = web {
                        extended_by_id.insert(id, (d, tags));
                    }
                }
            }
            Err(e) => tracing::warn!("could not build HTTP client for --extended: {e:#}"),
        }
    }

    // --- Render in the order the ids were requested ---
    if json {
        let items: Vec<serde_json::Value> = app_ids
            .iter()
            .filter_map(|id| {
                base.get(id)
                    .map(|(details, dlc)| info_json_value(details, dlc, extended_by_id.get(id)))
            })
            .collect();
        // One id keeps the original single-object shape (backward compatible);
        // several ids produce an array.
        if single {
            match items.into_iter().next() {
                Some(v) => cli_println!("{}", serde_json::to_string_pretty(&v)?),
                None => bail!("no store information available for app {}", app_ids[0]),
            }
        } else {
            cli_println!("{}", serde_json::to_string_pretty(&serde_json::Value::Array(items))?);
        }
        return Ok(());
    }

    let mut first = true;
    for id in &app_ids {
        let Some((details, dlc)) = base.get(id) else {
            continue;
        };
        if !first {
            cli_println!("\n{}", "─".repeat(60));
        }
        first = false;
        print_info_human(details, dlc, extended_by_id.get(id));
    }
    Ok(())
}

/// Build the `--json` object for one app from its CM metadata, DLC list and
/// optional `--extended` storefront data.
fn info_json_value(
    details: &StoreAppInfo,
    dlc: &[(u32, Option<String>)],
    extended_info: Option<&ExtendedInfo>,
) -> serde_json::Value {
    let mut value = serde_json::json!({
        "app_id": details.app_id,
        "name": details.name,
        "type": details.app_type,
        "is_free": details.is_free,
        "early_access": details.is_early_access,
        "description": details.short_description,
        "full_description": details.full_description,
        "developers": details.developers,
        "publishers": details.publishers,
        "franchises": details.franchises,
        "release_date": details.release_date,
        "coming_soon": details.coming_soon,
        "price": details.price,
        "discount_pct": details.discount_pct,
        "platforms": details.platforms,
        "reviews": details.review_summary,
        "assets": {
            "header": details.assets.header,
            "capsule": details.assets.capsule,
            "hero": details.assets.hero,
            "background": details.assets.background,
            "logo": details.assets.logo,
        },
        "dlc": dlc.iter().map(|(id, name)| serde_json::json!({"app_id": id, "name": name})).collect::<Vec<_>>(),
    });
    if let Some((web, tags)) = extended_info {
        value["extended"] = serde_json::json!({
            "genres": web.genres,
            "categories": web.categories,
            "tags": tags,
            "metacritic": web.metacritic,
            "website": web.website,
            "requirements": {
                "minimum": web.requirements_minimum,
                "recommended": web.requirements_recommended,
            },
        });
    }
    value
}

/// Render the human-readable `info` block for one app to stdout.
fn print_info_human(
    details: &StoreAppInfo,
    dlc: &[(u32, Option<String>)],
    extended_info: Option<&ExtendedInfo>,
) {
    // --- Header ---
    let ea = if details.is_early_access { " [Early Access]" } else { "" };
    cli_println!("{}  (app {}){ea}", details.name, details.app_id);
    if !details.app_type.is_empty() {
        cli_println!("Type       : {}", details.app_type);
    }
    if !details.developers.is_empty() {
        cli_println!("Developers : {}", details.developers.join(", "));
    }
    if !details.publishers.is_empty() {
        cli_println!("Publishers : {}", details.publishers.join(", "));
    }
    if !details.franchises.is_empty() {
        cli_println!("Franchises : {}", details.franchises.join(", "));
    }
    if let Some(date) = &details.release_date {
        let suffix = if details.coming_soon { " (coming soon)" } else { "" };
        cli_println!("Released   : {date}{suffix}");
    }
    if let Some(price) = &details.price {
        let discount = if details.discount_pct > 0 {
            format!(" (-{}%)", details.discount_pct)
        } else {
            String::new()
        };
        cli_println!("Price      : {price}{discount}");
    }
    if !details.platforms.is_empty() {
        cli_println!("Platforms  : {}", details.platforms.join(", "));
    }
    if let Some(reviews) = &details.review_summary {
        cli_println!("Reviews    : {reviews}");
    }
    if let Some((web, _)) = extended_info {
        if let Some(score) = web.metacritic {
            cli_println!("Metacritic : {score}");
        }
        if let Some(site) = &web.website {
            cli_println!("Website    : {site}");
        }
    }

    // --- Description ---
    if !details.short_description.is_empty() {
        cli_println!("\nDescription:");
        for line in wrap_text(&details.short_description, 88) {
            cli_println!("  {line}");
        }
    }

    // --- Extended: tags / genres / categories / requirements ---
    if let Some((web, tags)) = extended_info {
        if !tags.is_empty() {
            cli_println!(
                "\nTags      : {}",
                tags.iter().take(20).cloned().collect::<Vec<_>>().join(", ")
            );
        }
        if !web.genres.is_empty() {
            cli_println!("Genres    : {}", web.genres.join(", "));
        }
        if !web.categories.is_empty() {
            cli_println!("Categories: {}", web.categories.join(", "));
        }
        if !web.requirements_minimum.is_empty() {
            cli_println!("\nMinimum requirements:");
            for line in &web.requirements_minimum {
                cli_println!("  {line}");
            }
        }
        if !web.requirements_recommended.is_empty() {
            cli_println!("\nRecommended requirements:");
            for line in &web.requirements_recommended {
                cli_println!("  {line}");
            }
        }
    }

    // --- Artwork ---
    let a = &details.assets;
    if a.header.is_some() || a.capsule.is_some() || a.background.is_some() {
        cli_println!("\nArtwork:");
        if let Some(u) = &a.header {
            cli_println!("  header    : {u}");
        }
        if let Some(u) = &a.capsule {
            cli_println!("  capsule   : {u}");
        }
        if let Some(u) = &a.hero {
            cli_println!("  hero      : {u}");
        }
        if let Some(u) = &a.background {
            cli_println!("  background: {u}");
        }
        if let Some(u) = &a.logo {
            cli_println!("  logo      : {u}");
        }
    }

    // --- DLC ---
    if !dlc.is_empty() {
        cli_println!("\nDLC ({}):", dlc.len());
        for (id, name) in dlc {
            let name = name.clone().unwrap_or_else(|| "(name unavailable)".to_string());
            cli_println!("  {id:>9}  {name}");
        }
    }
}

/// Resolve DLC names via a single batched `StoreBrowse.GetItems` call (over the
/// Steam CM connection — no storefront API), returning `(app_id, name)` pairs
/// sorted by app id. A `None` name means the store didn't return that id.
async fn resolve_dlc_names_via_store(
    client: &SteamClient,
    dlc_ids: &[u32],
) -> Vec<(u32, Option<String>)> {
    if dlc_ids.is_empty() {
        return Vec::new();
    }
    let name_by_id: std::collections::HashMap<u32, String> = client
        .fetch_store_apps(dlc_ids)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|i| (i.app_id, i.name))
        .collect();

    let mut dlc: Vec<(u32, Option<String>)> = dlc_ids
        .iter()
        .map(|&id| (id, name_by_id.get(&id).cloned().filter(|s| !s.is_empty())))
        .collect();
    dlc.sort_by_key(|(id, _)| *id);
    dlc
}

async fn cmd_dlc(app_id: u32, json: bool) -> Result<()> {
    // Ownership status requires an authenticated connection; installed/disabled status
    // is read from the local appmanifest.
    let steam = authed_client().await?;

    // DLC ids come from PICS appinfo (`common.dlc`); names from a batched
    // StoreBrowse call — both over the Steam CM connection, no storefront API.
    let dlc_ids: Vec<u32> = steam
        .get_extended_app_info(app_id)
        .await
        .map(|e| e.dlcs)
        .unwrap_or_default();
    let dlc = resolve_dlc_names_via_store(&steam, &dlc_ids).await;
    let states = steam
        .dlc_states(app_id, &dlc_ids)
        .await
        .with_context(|| format!("failed to resolve DLC status for app {app_id}"))?;
    let state_by_id: std::collections::HashMap<u32, &aurelia::models::DlcState> =
        states.iter().map(|s| (s.app_id, s)).collect();

    if json {
        let value = serde_json::json!({
            "app_id": app_id,
            "dlc": dlc.iter().map(|(id, name)| {
                let s = state_by_id.get(id);
                serde_json::json!({
                    "app_id": id,
                    "name": name,
                    "owned": s.map(|s| s.owned),
                    "installed": s.map(|s| s.installed),
                    "disabled": s.map(|s| s.disabled),
                })
            }).collect::<Vec<_>>(),
        });
        cli_println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    if dlc.is_empty() {
        cli_println!("No DLC for app {app_id}.");
        return Ok(());
    }
    cli_println!("{:>9}  {:<5}  {:<13}  NAME", "APPID", "OWNED", "STATUS");
    for (id, name) in &dlc {
        let name = name.clone().unwrap_or_else(|| "(name unavailable)".to_string());
        let s = state_by_id.get(id);
        let owned = match s.map(|s| s.owned) {
            Some(true) => "yes",
            Some(false) => "no",
            None => "?",
        };
        // Installed/disabled describe the local content state of an owned DLC.
        let status = match s {
            Some(s) if !s.installed => "not-installed",
            Some(s) if s.disabled => "disabled",
            Some(_) => "enabled",
            None => "?",
        };
        cli_println!("{id:>9}  {owned:<5}  {status:<13}  {name}");
    }
    Ok(())
}

async fn cmd_achievements(app_id: u32, lang: String, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let achievements = client
        .fetch_achievements(app_id, &lang)
        .await
        .with_context(|| format!("failed to fetch achievements for app {app_id}"))?;
    let unlocked = achievements.iter().filter(|a| a.unlocked).count();

    if json {
        let arr: Vec<_> = achievements
            .iter()
            .map(|a| {
                serde_json::json!({
                    "achievement_id": a.api_name,
                    "achievement_key": a.api_name,
                    "name": a.name,
                    "description": a.description,
                    "visible": !a.hidden,
                    "image_url_unlocked": a.icon_unlocked,
                    "image_url_locked": a.icon_locked,
                    "rarity": a.global_percent,
                    "unlocked": a.unlocked,
                    "unlock_time": a.unlock_time,
                    "date_unlocked": a.unlock_time.map(|t| format_unix_timestamp(u64::from(t))),
                })
            })
            .collect();
        print_json(&serde_json::json!({
            "app_id": app_id,
            "unlocked": unlocked,
            "total": achievements.len(),
            "achievements": arr,
        }));
        return Ok(());
    }

    if achievements.is_empty() {
        cli_println!("No achievements for app {app_id}.");
        return Ok(());
    }
    cli_println!("{:<2}  {:>6}  {:<19}  NAME", "", "RARITY", "UNLOCKED");
    for a in &achievements {
        let mark = if a.unlocked { "✓" } else { " " };
        let when = a
            .unlock_time
            .map(|t| format_unix_timestamp(u64::from(t)))
            .unwrap_or_else(|| "-".to_string());
        let name = if a.hidden && !a.unlocked {
            format!("{} (hidden)", a.name)
        } else {
            a.name.clone()
        };
        cli_println!("{mark:<2}  {:>5.1}%  {when:<19}  {name}", a.global_percent);
    }
    cli_println!("\n{unlocked}/{} unlocked.", achievements.len());
    Ok(())
}

async fn cmd_depots(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let depots = client
        .get_depot_list(app_id)
        .await
        .with_context(|| format!("failed to load depots for app {app_id}"))?;

    if json {
        let arr: Vec<_> = depots
            .iter()
            .map(|d| {
                serde_json::json!({
                    "id": d.id,
                    "name": d.name,
                    "size": d.size,
                    "file_count": d.file_count,
                    "config": d.config,
                    "is_owned": d.is_owned,
                })
            })
            .collect();
        print_json(&serde_json::json!({ "app_id": app_id, "depots": arr }));
        return Ok(());
    }

    if depots.is_empty() {
        cli_println!("No depots reported.");
        return Ok(());
    }
    cli_println!("{:>12}  {:>14}  NAME", "DEPOT", "SIZE(bytes)");
    for d in &depots {
        cli_println!("{:>12}  {:>14}  {}", d.id, d.size, d.name);
    }
    Ok(())
}

async fn cmd_launch_options(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let options = client
        .fetch_launch_options(app_id)
        .await
        .with_context(|| format!("failed to load launch options for app {app_id}"))?;

    if json {
        let arr: Vec<_> = options
            .iter()
            .map(|o| {
                serde_json::json!({
                    "id": o.id,
                    "description": o.description,
                    "executable": o.executable,
                    "arguments": o.arguments,
                    "working_dir": o.working_dir,
                    "oslist": o.oslist,
                    "osarch": o.osarch,
                    "type": o.launch_type,
                })
            })
            .collect();
        print_json(&serde_json::json!({ "app_id": app_id, "launch_options": arr }));
        return Ok(());
    }

    if options.is_empty() {
        cli_println!("No launch options for app {app_id}.");
        return Ok(());
    }
    cli_println!("{:>3}  {:<10}  NAME / COMMAND", "ID", "OS");
    for o in &options {
        let os = if o.oslist.is_empty() { "any" } else { &o.oslist };
        let desc = if o.description.is_empty() {
            &o.executable
        } else {
            &o.description
        };
        cli_println!("{:>3}  {:<10}  {}", o.id, os, desc);
        let cmd = format!("{} {}", o.executable, o.arguments);
        let cmd = cmd.trim();
        if !cmd.is_empty() {
            cli_println!("       {cmd}");
        }
    }
    Ok(())
}

async fn cmd_image(app_id: u32, output: Option<PathBuf>, force: bool, json: bool) -> Result<()> {
    let cache_dir = aurelia::config::opensteam_image_cache_dir()?;
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .with_context(|| format!("failed creating {}", cache_dir.display()))?;
    let cache_path = cache_dir.join(format!("{app_id}_library.jpg"));

    if force || tokio::fs::metadata(&cache_path).await.is_err() {
        // Steam CDN artwork variants, tried in order of preference.
        let candidates = [
            format!("https://cdn.akamai.steamstatic.com/steam/apps/{app_id}/library_600x900_2x.jpg"),
            format!("https://cdn.akamai.steamstatic.com/steam/apps/{app_id}/header.jpg"),
            format!("https://steamcdn-a.akamaihd.net/steam/apps/{app_id}/library_capsule_2x.jpg"),
        ];

        let mut downloaded = false;
        for url in candidates {
            match reqwest::get(&url).await {
                Ok(resp) if resp.status().is_success() => {
                    let bytes = resp.bytes().await.context("failed reading image bytes")?;
                    tokio::fs::write(&cache_path, &bytes)
                        .await
                        .with_context(|| format!("failed writing {}", cache_path.display()))?;
                    downloaded = true;
                    break;
                }
                _ => continue,
            }
        }

        if !downloaded {
            bail!("no artwork found on the Steam CDN for app {app_id}");
        }
    }

    let final_path = match output {
        Some(out) => {
            tokio::fs::copy(&cache_path, &out)
                .await
                .with_context(|| format!("failed writing {}", out.display()))?;
            out
        }
        None => cache_path,
    };
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "path": final_path.display().to_string(),
        }));
    } else {
        cli_println!("{}", final_path.display());
    }
    Ok(())
}

async fn cmd_config_show(_json: bool) -> Result<()> {
    // The launcher configuration is structured data; it always renders as JSON.
    let config = load_launcher_config().await.unwrap_or_default();
    cli_println!("{}", serde_json::to_string_pretty(&config)?);
    Ok(())
}

fn cmd_config_protons(json: bool) -> Result<()> {
    let (steam, custom) = scan_proton_runtimes();
    if json {
        print_json(&serde_json::json!({ "steam": steam, "custom": custom }));
        return Ok(());
    }
    cli_println!("Steam runtimes:");
    for s in &steam {
        cli_println!("  {s}");
    }
    if !custom.is_empty() {
        cli_println!("Custom (compatibilitytools.d):");
        for c in &custom {
            cli_println!("  {c}");
        }
    }
    Ok(())
}

async fn cmd_cloud_sync(
    app_id: u32,
    up: bool,
    down: bool,
    path: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;
    let cloud = client.cloud_client()?;

    // Classic (token-less) remote-storage files live under `<appid>/remote`; Auto-Cloud
    // files resolve to real OS paths via their `%RootToken%` prefix. `--path` overrides
    // only the classic base. `%GameInstall%` needs the game's install directory.
    let remote_root = match path {
        Some(p) => p,
        None => aurelia::cloud_sync::default_cloud_root(cloud.steam_id(), app_id)
            .context("could not resolve the local cloud save directory")?
            .join("remote"),
    };
    let (_, install_path) = client.is_game_available(app_id).await;
    let resolver =
        aurelia::cloud_sync::CloudPathResolver::new(remote_root.clone(), install_path.map(PathBuf::from));

    // No flag = full sync (down then up); `--down`/`--up` restrict the direction.
    let mut downloaded = false;
    let mut uploaded = false;
    if !up {
        cloud
            .sync_down(app_id, &resolver)
            .await
            .with_context(|| format!("cloud sync-down failed for app {app_id}"))?;
        downloaded = true;
    }
    if !down {
        // UFS save rules let sync_up discover brand-new local saves; best-effort.
        let specs = client.fetch_ufs_save_specs(app_id).await.unwrap_or_default();
        cloud
            .sync_up(app_id, &resolver, &specs)
            .await
            .with_context(|| format!("cloud sync-up failed for app {app_id}"))?;
        uploaded = true;
    }

    let direction = if up {
        "up"
    } else if down {
        "down"
    } else {
        "both"
    };
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "direction": direction,
            "remote_root": remote_root.to_string_lossy(),
            "downloaded": downloaded,
            "uploaded": uploaded,
        }));
    } else {
        cli_println!("Synced Cloud saves for app {app_id} ({direction}).");
    }
    Ok(())
}

async fn cmd_cloud_list(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let cloud = client.cloud_client()?;
    let mut files = cloud
        .get_file_list(app_id)
        .await
        .with_context(|| format!("failed listing Cloud files for app {app_id}"))?;
    files.sort_by(|a, b| a.filename.cmp(&b.filename));

    if json {
        let arr: Vec<_> = files
            .iter()
            .map(|f| {
                serde_json::json!({
                    "filename": f.filename,
                    "size": f.size,
                    "timestamp": f.timestamp,
                    "sha_hash": f.sha_hash,
                })
            })
            .collect();
        print_json(&serde_json::json!({ "app_id": app_id, "files": arr }));
        return Ok(());
    }

    if files.is_empty() {
        cli_println!("No Steam Cloud files for app {app_id}.");
        return Ok(());
    }
    cli_println!("{:>12}  {:<19}  NAME", "SIZE", "MODIFIED");
    let mut total = 0u64;
    for f in &files {
        total += f.size;
        cli_println!(
            "{:>12}  {:<19}  {}",
            human_bytes(f.size),
            format_unix_timestamp(f.timestamp),
            f.filename
        );
    }
    cli_println!("\n{} file(s), {}.", files.len(), human_bytes(total));
    Ok(())
}

/// Human label for a Workshop entry kind.
fn workshop_kind_label(kind: aurelia::models::WorkshopItemKind) -> &'static str {
    match kind {
        aurelia::models::WorkshopItemKind::Collection => "collection",
        aurelia::models::WorkshopItemKind::Item => "item",
    }
}

/// `workshop browse`: search/browse a game's Workshop to discover items.
async fn cmd_workshop_browse(
    app_id: u32,
    search: Option<String>,
    sort: WorkshopSort,
    count: u32,
    cursor: String,
    tags: Vec<String>,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;
    let page = client
        .query_workshop_files(app_id, search.as_deref(), sort.query_type(), &cursor, count, &tags)
        .await?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "total": page.total,
            "next_cursor": page.next_cursor,
            "items": page.items,
        }));
        return Ok(());
    }

    if page.items.is_empty() {
        cli_println!("No Workshop items found.");
        return Ok(());
    }
    cli_println!("{:>12}  {:>10}  TITLE", "ID", "SIZE");
    for item in &page.items {
        let size = if item.file_size > 0 {
            human_bytes(item.file_size)
        } else {
            "-".to_string()
        };
        cli_println!("{:>12}  {:>10}  {}", item.id, size, item.title);
    }
    cli_println!("\nShowing {} of {} result(s).", page.items.len(), page.total);
    // Offer the next page only when the cursor actually advances.
    if !page.next_cursor.is_empty() && page.next_cursor != cursor {
        cli_println!("Next page: --cursor \"{}\"", page.next_cursor);
    }
    Ok(())
}

/// `workshop info`: show metadata for one or more published files.
async fn cmd_workshop_info(ids: Vec<u64>, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let items = client.fetch_published_file_details(&ids).await?;

    if json {
        cli_println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }
    if items.is_empty() {
        cli_println!("No Workshop items found.");
        return Ok(());
    }
    for item in &items {
        cli_println!("ID      : {}", item.id);
        cli_println!("Title   : {}", item.title);
        cli_println!("App     : {}", item.app_id);
        cli_println!("Type    : {}", workshop_kind_label(item.kind));
        if item.kind == aurelia::models::WorkshopItemKind::Collection {
            cli_println!("Items   : {}", item.children.len());
        } else {
            cli_println!("Size    : {}", human_bytes(item.file_size));
            cli_println!("Manifest: {}", item.hcontent_file);
        }
        cli_println!("Updated : {}", format_unix_timestamp(item.time_updated.max(0) as u64));
        cli_println!();
    }
    Ok(())
}

/// `workshop list`: the items you're subscribed to for a game.
async fn cmd_workshop_list(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let ids = client.fetch_subscribed_items(app_id).await?;
    let items = client.fetch_published_file_details(&ids).await?;

    if json {
        cli_println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }
    if items.is_empty() {
        cli_println!("No subscribed Workshop items for app {app_id}.");
        return Ok(());
    }
    cli_println!("{:>12}  {:>12}  TITLE", "ID", "SIZE");
    for item in &items {
        cli_println!("{:>12}  {:>12}  {}", item.id, human_bytes(item.file_size), item.title);
    }
    cli_println!("\n{} subscribed item(s).", items.len());
    Ok(())
}

/// Expand collection ids to leaf item ids unless `no_recurse`.
async fn workshop_resolve_ids(
    client: &SteamClient,
    ids: Vec<u64>,
    no_recurse: bool,
) -> Result<Vec<u64>> {
    if no_recurse {
        Ok(ids)
    } else {
        client.expand_collections(&ids).await
    }
}

/// `workshop install`: download item(s)/collection(s) and register them.
async fn cmd_workshop_install(ids: Vec<u64>, no_recurse: bool, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let leaf_ids = workshop_resolve_ids(&client, ids, no_recurse).await?;
    let items = client.fetch_published_file_details(&leaf_ids).await?;
    if items.is_empty() {
        bail!("no installable Workshop items resolved");
    }

    for item in &items {
        if !json {
            cli_println!("Installing Workshop item {} ({}) ...", item.id, item.title);
        }
        let state = Arc::new(RwLock::new(DownloadState::default()));
        let rx = client
            .install_workshop_item(item, state)
            .await
            .with_context(|| format!("failed to start install for Workshop item {}", item.id))?;
        drive_progress(rx, json).await?;
        if json {
            print_json_line(&serde_json::json!({
                "event": "result",
                "id": item.id,
                "app_id": item.app_id,
                "status": "installed",
            }));
        }
    }
    Ok(())
}

/// `workshop uninstall`: remove installed item(s)/collection(s).
async fn cmd_workshop_uninstall(ids: Vec<u64>, no_recurse: bool, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let leaf_ids = workshop_resolve_ids(&client, ids, no_recurse).await?;
    // GetDetails resolves each item's owning app id (needed to find its files).
    let items = client.fetch_published_file_details(&leaf_ids).await?;

    let mut removed = Vec::new();
    for item in &items {
        client
            .uninstall_workshop_item(item.id, item.app_id)
            .await
            .with_context(|| format!("failed to uninstall Workshop item {}", item.id))?;
        removed.push(item.id);
        if !json {
            cli_println!("Uninstalled Workshop item {} ({}).", item.id, item.title);
        }
    }
    if json {
        print_json(&serde_json::json!({ "uninstalled": removed }));
    } else if removed.is_empty() {
        cli_println!("Nothing to uninstall.");
    }
    Ok(())
}

/// `workshop subscribe`: subscribe (and optionally install) item(s)/collection(s).
async fn cmd_workshop_subscribe(
    ids: Vec<u64>,
    install: bool,
    no_recurse: bool,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;
    let leaf_ids = workshop_resolve_ids(&client, ids, no_recurse).await?;
    let items = client.fetch_published_file_details(&leaf_ids).await?;

    let mut subscribed = Vec::new();
    for item in &items {
        client
            .subscribe_published_file(item.id, item.app_id)
            .await
            .with_context(|| format!("failed to subscribe to Workshop item {}", item.id))?;
        subscribed.push(item.id);
        if !json {
            cli_println!("Subscribed to Workshop item {} ({}).", item.id, item.title);
        }
    }

    if install {
        for item in &items {
            if !json {
                cli_println!("Installing Workshop item {} ...", item.id);
            }
            let state = Arc::new(RwLock::new(DownloadState::default()));
            let rx = client
                .install_workshop_item(item, state)
                .await
                .with_context(|| format!("failed to start install for Workshop item {}", item.id))?;
            drive_progress(rx, json).await?;
        }
    }

    if json {
        print_json(&serde_json::json!({ "subscribed": subscribed, "installed": install }));
    }
    Ok(())
}

/// `workshop unsubscribe`: unsubscribe from item(s)/collection(s).
async fn cmd_workshop_unsubscribe(ids: Vec<u64>, no_recurse: bool, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let leaf_ids = workshop_resolve_ids(&client, ids, no_recurse).await?;
    let items = client.fetch_published_file_details(&leaf_ids).await?;

    let mut unsubscribed = Vec::new();
    for item in &items {
        client
            .unsubscribe_published_file(item.id, item.app_id)
            .await
            .with_context(|| format!("failed to unsubscribe from Workshop item {}", item.id))?;
        unsubscribed.push(item.id);
        if !json {
            cli_println!("Unsubscribed from Workshop item {} ({}).", item.id, item.title);
        }
    }
    if json {
        print_json(&serde_json::json!({ "unsubscribed": unsubscribed }));
    }
    Ok(())
}

/// `workshop status`: installed vs subscribed (with update detection) for a game.
async fn cmd_workshop_status(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let installed = client.read_installed_workshop(app_id).await?;
    // Subscriptions and live manifests need the network; treat them as best-effort
    // so `status` still reports the local install set when offline.
    let subscribed = client.fetch_subscribed_items(app_id).await.unwrap_or_default();

    // Union of installed + subscribed ids, to fetch current manifests for update detection.
    let mut all_ids: Vec<u64> = installed.iter().map(|i| i.id).collect();
    for &s in &subscribed {
        if !all_ids.contains(&s) {
            all_ids.push(s);
        }
    }
    let details = client
        .fetch_published_file_details(&all_ids)
        .await
        .unwrap_or_default();
    let current_manifest = |id: u64| -> Option<u64> {
        details.iter().find(|d| d.id == id).map(|d| d.hcontent_file)
    };
    let title_of = |id: u64| -> String {
        details
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.title.clone())
            .unwrap_or_default()
    };

    let is_installed = |id: u64| installed.iter().find(|i| i.id == id);

    if json {
        let arr: Vec<_> = all_ids
            .iter()
            .map(|&id| {
                let inst = is_installed(id);
                let update = match (inst, current_manifest(id)) {
                    (Some(i), Some(cur)) => cur != 0 && cur != i.manifest_id,
                    _ => false,
                };
                serde_json::json!({
                    "id": id,
                    "title": title_of(id),
                    "installed": inst.is_some(),
                    "subscribed": subscribed.contains(&id),
                    "update_available": update,
                })
            })
            .collect();
        print_json(&serde_json::json!({ "app_id": app_id, "items": arr }));
        return Ok(());
    }

    if all_ids.is_empty() {
        cli_println!("No installed or subscribed Workshop items for app {app_id}.");
        return Ok(());
    }
    cli_println!(
        "{:>12}  {:<9}  {:<10}  {:<7}  TITLE",
        "ID", "INSTALLED", "SUBSCRIBED", "UPDATE"
    );
    for &id in &all_ids {
        let inst = is_installed(id);
        let update = match (inst, current_manifest(id)) {
            (Some(i), Some(cur)) => cur != 0 && cur != i.manifest_id,
            _ => false,
        };
        cli_println!(
            "{:>12}  {:<9}  {:<10}  {:<7}  {}",
            id,
            if inst.is_some() { "yes" } else { "no" },
            if subscribed.contains(&id) { "yes" } else { "no" },
            if update { "yes" } else { "-" },
            title_of(id),
        );
    }
    Ok(())
}

/// `workshop rate`: thumbs-up/down a Workshop item.
async fn cmd_workshop_rate(id: u64, up: bool, json: bool) -> Result<()> {
    let client = authed_client().await?;
    client.vote_workshop_item(id, up).await?;
    if json {
        print_json(&serde_json::json!({
            "id": id,
            "vote": if up { "up" } else { "down" },
            "status": "rated",
        }));
    } else {
        cli_println!(
            "Rated Workshop item {id} {}.",
            if up { "thumbs-up" } else { "thumbs-down" }
        );
    }
    Ok(())
}

/// `workshop comments`: read a page of a Workshop item's comments.
async fn cmd_workshop_comments_read(id: u64, start: i32, count: i32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let comments = client.workshop_comments(id, start, count).await?;

    if json {
        print_json(&serde_json::json!({ "id": id, "comments": comments }));
        return Ok(());
    }
    if comments.is_empty() {
        cli_println!("No comments on Workshop item {id}.");
        return Ok(());
    }
    for c in &comments {
        cli_println!(
            "[{}] {} (+{})",
            format_unix_timestamp(c.timestamp.max(0) as u64),
            c.author,
            c.upvotes
        );
        cli_println!("  {}", c.text);
    }
    cli_println!("\n{} comment(s).", comments.len());
    Ok(())
}

/// `workshop comment`: post a comment to a Workshop item.
async fn cmd_workshop_comment(id: u64, text: String, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let comment_id = client.post_workshop_comment(id, &text).await?;
    if json {
        print_json(&serde_json::json!({
            "id": id,
            "comment_id": comment_id,
            "status": "posted",
        }));
    } else {
        cli_println!("Posted comment {comment_id} to Workshop item {id}.");
    }
    Ok(())
}

/// Format a Unix timestamp (seconds) as `YYYY-MM-DD HH:MM:SS` (UTC).
fn format_unix_timestamp(secs: u64) -> String {
    let tod = secs % 86_400;
    let (h, m, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    format!(
        "{} {:02}:{:02}:{:02}",
        aurelia::steam_client::unix_to_ymd(secs as i64),
        h,
        m,
        s
    )
}

/// Consume a download/verify progress stream, rendering it to the terminal.
/// In JSON mode each update is emitted as a compact NDJSON line (one object per
/// line) on stdout; the caller still prints the final result object afterward.
async fn drive_progress(
    mut rx: tokio::sync::mpsc::Receiver<DownloadProgress>,
    json: bool,
) -> Result<()> {
    // De-duplicate identical consecutive JSON events (the reporter ticks on a timer,
    // so it would otherwise repeat e.g. "0/0" lines while a manifest is being fetched).
    let mut last: Option<(u8, u64, u64)> = None;
    let mut rate = RateTracker::new();
    while let Some(p) = rx.recv().await {
        match p.state {
            DownloadProgressState::Queued => {
                rate.reset();
                if json {
                    emit_progress_json("queued", &p, 0.0, None, &mut last);
                } else {
                    cli_println!("Queued ...");
                }
            }
            DownloadProgressState::Downloading
            | DownloadProgressState::Verifying
            | DownloadProgressState::Moving => {
                let (state, label) = match p.state {
                    DownloadProgressState::Verifying => ("verifying", "Verifying"),
                    DownloadProgressState::Moving => ("moving", "Moving"),
                    _ => ("downloading", "Downloading"),
                };
                let (speed, eta) = rate.sample(p.bytes_downloaded, p.total_bytes);
                if json {
                    emit_progress_json(state, &p, speed, eta, &mut last);
                } else {
                    print_progress(label, &p, speed, eta);
                }
            }
            DownloadProgressState::Completed => {
                // The caller emits the terminal result object; nothing more here.
                if !json {
                    cli_println!("\nDone.");
                }
                return Ok(());
            }
            DownloadProgressState::Failed => {
                if !json {
                    cli_println!();
                }
                bail!("operation failed: {}", p.current_file);
            }
        }
    }
    Ok(())
}

/// Percentage (one decimal) of `done` out of `total`, 0 when total is unknown.
fn percent_of(done: u64, total: u64) -> f64 {
    if total > 0 {
        ((done as f64 / total as f64) * 1000.0).round() / 10.0
    } else {
        0.0
    }
}

/// Emit one compact NDJSON progress event, skipping it if identical to the last.
/// Reports the whole-app progress (`percent`), the current depot's progress
/// (`depot_percent`), and the transfer rate (`speed_bps`, bytes/sec) and
/// `eta_seconds` (null when not yet estimable).
fn emit_progress_json(
    state: &str,
    p: &DownloadProgress,
    speed_bps: f64,
    eta_seconds: Option<u64>,
    last: &mut Option<(u8, u64, u64)>,
) {
    // Cheap discriminator for the state so we can dedupe (state, overall, depot).
    let state_key = match state {
        "queued" => 0u8,
        "downloading" => 1,
        "verifying" => 2,
        "moving" => 3,
        _ => 4,
    };
    let key = (state_key, p.bytes_downloaded, p.depot_bytes_downloaded);
    if *last == Some(key) {
        return;
    }
    *last = Some(key);

    let value = serde_json::json!({
        "event": "progress",
        "state": state,
        // Whole-app (all depots) progress.
        "bytes_downloaded": p.bytes_downloaded,
        "total_bytes": p.total_bytes,
        "percent": percent_of(p.bytes_downloaded, p.total_bytes),
        // Current depot progress.
        "depot_id": p.depot_id,
        "depot_bytes_downloaded": p.depot_bytes_downloaded,
        "depot_total_bytes": p.depot_total_bytes,
        "depot_percent": percent_of(p.depot_bytes_downloaded, p.depot_total_bytes),
        // Rate / time remaining (for a download-manager progress bar).
        "speed_bps": speed_bps.round() as u64,
        "eta_seconds": eta_seconds,
        "file": p.current_file,
    });
    // Compact single line so the whole --json stream is valid NDJSON.
    if let Ok(s) = serde_json::to_string(&value) {
        cli_println!("{s}");
    }
}

fn print_progress(phase: &str, p: &DownloadProgress, speed_bps: f64, eta_seconds: Option<u64>) {
    let overall = percent_of(p.bytes_downloaded, p.total_bytes);
    let rate = format_rate(speed_bps, eta_seconds);
    if p.depot_id != 0 {
        let depot = percent_of(p.depot_bytes_downloaded, p.depot_total_bytes);
        cli_print!(
            "\r{phase}: {overall:5.1}% overall  {}/{} bytes  | depot {}: {depot:5.1}%{rate}   ",
            p.bytes_downloaded, p.total_bytes, p.depot_id
        );
    } else {
        cli_print!(
            "\r{phase}: {overall:5.1}%  {}/{} bytes{rate}  {}   ",
            p.bytes_downloaded, p.total_bytes, p.current_file
        );
    }
    let _ = std::io::stdout().flush();
}

/// Tracks the transfer rate across successive progress samples, deriving a lightly
/// smoothed speed (bytes/sec) and an ETA. Used by `drive_progress` so every
/// long-running op (download/verify/move) reports speed and time remaining.
struct RateTracker {
    last: Option<(std::time::Instant, u64)>,
    speed_bps: f64,
}

impl RateTracker {
    fn new() -> Self {
        Self { last: None, speed_bps: 0.0 }
    }

    fn reset(&mut self) {
        self.last = None;
        self.speed_bps = 0.0;
    }

    /// Feed the latest cumulative `bytes` (out of `total`); returns
    /// `(speed_bps, eta_seconds)`. Samples closer together than 100 ms are folded
    /// into the next interval to keep the estimate stable.
    fn sample(&mut self, bytes: u64, total: u64) -> (f64, Option<u64>) {
        let now = std::time::Instant::now();
        match self.last {
            Some((t0, b0)) => {
                let dt = now.duration_since(t0).as_secs_f64();
                if dt >= 0.10 && bytes >= b0 {
                    let inst = (bytes - b0) as f64 / dt;
                    // Exponential moving average to damp jitter.
                    self.speed_bps = if self.speed_bps <= 0.0 {
                        inst
                    } else {
                        0.6 * self.speed_bps + 0.4 * inst
                    };
                    self.last = Some((now, bytes));
                }
            }
            None => self.last = Some((now, bytes)),
        }
        let eta = if self.speed_bps > 1.0 && total > bytes {
            Some(((total - bytes) as f64 / self.speed_bps).round() as u64)
        } else {
            None
        };
        (self.speed_bps, eta)
    }
}

/// Human-readable ` 12.34 MiB/s  ETA 00:01:23` suffix (empty when no rate yet).
fn format_rate(speed_bps: f64, eta_seconds: Option<u64>) -> String {
    if speed_bps <= 0.0 {
        return String::new();
    }
    let mib = speed_bps / (1024.0 * 1024.0);
    match eta_seconds {
        Some(s) => format!("  {mib:6.2} MiB/s  ETA {}", format_eta(s)),
        None => format!("  {mib:6.2} MiB/s"),
    }
}

/// Format a seconds count as `HH:MM:SS`.
fn format_eta(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// Word-wrap text to a maximum line width, preserving existing line breaks.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
            } else if current.chars().count() + 1 + word.chars().count() <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(std::mem::take(&mut current));
                current.push_str(word);
            }
        }
        lines.push(current);
    }
    lines
}

fn prompt_line(prompt: &str) -> Result<String> {
    // Write the prompt to stderr so stdout stays clean (important for --json).
    cli_eprint!("{prompt}");
    std::io::stderr().flush().ok();
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("failed reading input")?;
    Ok(input.trim().to_string())
}

/// Discover Proton/Wine runtimes under the user's Steam directories.
fn scan_proton_runtimes() -> (Vec<String>, Vec<String>) {
    let mut steam = vec!["experimental".to_string()];
    let mut custom = Vec::new();

    if let Ok(home) = aurelia::config::home_dir() {
        let steam_tools = home.join(".local/share/Steam/steamapps/common");
        let custom_tools = home.join(".local/share/Steam/compatibilitytools.d");

        if let Ok(entries) = std::fs::read_dir(steam_tools) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.to_ascii_lowercase().contains("proton") {
                        steam.push(name);
                    }
                }
            }
        }

        if let Ok(entries) = std::fs::read_dir(custom_tools) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    custom.push(entry.file_name().to_string_lossy().to_string());
                }
            }
        }
    }

    steam.sort();
    steam.dedup();
    custom.sort();
    custom.dedup();
    (steam, custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_scales_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.00 KiB");
        assert_eq!(human_bytes(1536), "1.50 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.00 GiB");
    }

    #[test]
    fn eta_formats_hms() {
        assert_eq!(format_eta(0), "00:00:00");
        assert_eq!(format_eta(83), "00:01:23");
        assert_eq!(format_eta(3661), "01:01:01");
    }

    #[test]
    fn rate_tracker_estimates_speed_and_eta() {
        let mut r = RateTracker::new();
        // First sample only primes the tracker (no prior point).
        let (s0, e0) = r.sample(0, 1000);
        assert_eq!(s0, 0.0);
        assert!(e0.is_none());
        // Force a measurable interval, then feed more bytes.
        std::thread::sleep(std::time::Duration::from_millis(120));
        let (s1, e1) = r.sample(200, 1000);
        assert!(s1 > 0.0, "speed should be positive after progress");
        assert!(e1.is_some(), "eta should be estimable once moving");
    }

    #[test]
    fn rate_tracker_resets() {
        let mut r = RateTracker::new();
        let _ = r.sample(100, 1000);
        r.reset();
        let (s, e) = r.sample(0, 1000);
        assert_eq!(s, 0.0);
        assert!(e.is_none());
    }

    /// Parse an argv into a `Cli` the way the binary does.
    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("args should parse")
    }

    #[test]
    fn interactive_login_runs_locally_not_forwarded() {
        // The regression: these were forwarded to the daemon, where rpassword ran
        // without a tty and the password was echoed in clear text.
        assert!(must_run_locally(&parse(&["aurelia", "login"])));
        assert!(must_run_locally(&parse(&["aurelia", "login", "-u", "me"])));
        assert!(must_run_locally(&parse(&["aurelia", "login", "--qr"])));
        assert!(must_run_locally(&parse(&["aurelia", "login", "--code"])));
        assert!(must_run_locally(&parse(&["aurelia", "login", "--pin"])));
    }

    #[test]
    fn daemon_oriented_and_json_login_still_forward() {
        // These need the daemon (or are non-tty), so they must NOT be pinned local.
        assert!(!must_run_locally(&parse(&["aurelia", "login", "--health"])));
        assert!(!must_run_locally(&parse(&["aurelia", "login", "--reconnect"])));
        assert!(!must_run_locally(&parse(&["aurelia", "--json", "login", "-u", "me"])));
        assert!(!must_run_locally(&parse(&["aurelia", "login", "--json", "--qr"])));
    }

    #[test]
    fn ordinary_commands_forward_but_local_managers_do_not() {
        assert!(!must_run_locally(&parse(&["aurelia", "list"])));
        assert!(!must_run_locally(&parse(&["aurelia", "logout"])));
        // Process/daemon managers must run in-process.
        assert!(must_run_locally(&parse(&["aurelia", "kill"])));
        assert!(must_run_locally(&parse(&["aurelia", "daemon", "stop"])));
        // Bare `aurelia daemon` (becomes the server) is handled earlier, before the
        // forward gate, so it is not flagged local here.
        assert!(!must_run_locally(&parse(&["aurelia", "daemon"])));
    }
}
