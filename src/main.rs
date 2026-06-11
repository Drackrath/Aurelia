use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use aurelia::config::{
    load_launcher_config, load_library_cache, load_session, load_user_configs,
};
use aurelia::library::{build_game_library, scan_installed_app_info};
use aurelia::models::{
    DepotPlatform, DownloadProgress, DownloadProgressState, DownloadState, LibraryGame,
};
use aurelia::steam_client::{SharedApp, SteamClient};

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
    },
    /// Uninstall a game.
    Uninstall {
        app_id: u32,
        /// Also delete the game's Wine prefix / compat data.
        #[arg(long)]
        delete_prefix: bool,
    },
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
    /// Show detailed information about a game (description, tags, categories, DLC).
    Info { app_id: u32 },
    /// List a game's DLC (app id and name only).
    Dlc { app_id: u32 },
    /// List depots for a game.
    Depots { app_id: u32 },
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
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Print the current launcher configuration as JSON.
    Show,
    /// List detected Proton/Wine runtimes.
    Protons,
}

#[derive(Clone, Copy, ValueEnum)]
enum PlatformArg {
    Windows,
    Linux,
}

impl From<PlatformArg> for DepotPlatform {
    fn from(value: PlatformArg) -> Self {
        match value {
            PlatformArg::Windows => DepotPlatform::Windows,
            PlatformArg::Linux => DepotPlatform::Linux,
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let json = cli.json;

    // Send tracing/diagnostics to stderr so stdout stays clean for --json output.
    aurelia::infra::logging::init_cli_logging(cli.verbose);

    if let Err(err) = run(cli).await {
        if json {
            // Single line so it stays valid in the NDJSON streams (e.g. `install`).
            print_json_line(&serde_json::json!({ "error": format!("{err:#}") }));
        } else {
            eprintln!("Error: {err:#}");
        }
        std::process::exit(1);
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
        } => cmd_login(username, password, guard, qr, code, json).await,
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
        } => cmd_install(app_id, platform, restart_steam, json).await,
        Command::Uninstall {
            app_id,
            delete_prefix,
        } => cmd_uninstall(app_id, delete_prefix, json).await,
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
        Command::Info { app_id } => cmd_info(app_id, json).await,
        Command::Dlc { app_id } => cmd_dlc(app_id, json).await,
        Command::Depots { app_id } => cmd_depots(app_id, json).await,
        Command::Image {
            app_id,
            output,
            force,
        } => cmd_image(app_id, output, force, json).await,
        Command::Config { command } => match command {
            ConfigCommand::Show => cmd_config_show(json).await,
            ConfigCommand::Protons => cmd_config_protons(json),
        },
    }
}

/// Print a JSON value to stdout (pretty-printed).
fn print_json(value: &serde_json::Value) {
    match serde_json::to_string_pretty(value) {
        Ok(s) => println!("{s}"),
        Err(_) => println!("{{}}"),
    }
}

/// Print a JSON value as a single compact line (for NDJSON streams).
fn print_json_line(value: &serde_json::Value) {
    match serde_json::to_string(value) {
        Ok(s) => println!("{s}"),
        Err(_) => println!("{{}}"),
    }
}

/// Build a client and restore a persisted session if one exists.
async fn restored_client() -> Result<SteamClient> {
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

async fn cmd_login(
    username: Option<String>,
    password: Option<String>,
    guard: Option<String>,
    qr: bool,
    code: bool,
    json: bool,
) -> Result<()> {
    if qr {
        return cmd_login_qr(json).await;
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
async fn cmd_login_qr(json: bool) -> Result<()> {
    let mut client = SteamClient::new()?;
    let session = client
        .login_qr(render_login_qr)
        .await
        .context("QR login failed")?;
    let account = session.account_name.clone().unwrap_or_default();
    report_login_success(&account, json);
    Ok(())
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
            eprintln!("\nScan this QR code with the Steam Mobile app:\n{rendered}");
        }
        Err(e) => eprintln!("\n(could not render QR code: {e})"),
    }
    eprintln!("Or open this link in the Steam Mobile app:\n  {url}\n");
}

fn report_login_success(account: &str, json: bool) {
    if json {
        print_json(&serde_json::json!({ "logged_in": true, "account": account }));
    } else {
        println!("Login successful.");
    }
}

async fn cmd_logout(json: bool) -> Result<()> {
    let mut client = restored_client().await?;
    client.logout().await?;
    if json {
        print_json(&serde_json::json!({ "logged_out": true }));
    } else {
        println!("Logged out.");
    }
    Ok(())
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
        println!("{}", serde_json::to_string_pretty(&games)?);
        return Ok(());
    }

    if games.is_empty() {
        println!("No games match.");
        return Ok(());
    }

    if online {
        println!(
            "{:>9}  {:<10}  {:<13}  {:<7}  NAME",
            "APPID", "STATUS", "LICENSE", "ONLINE"
        );
    } else {
        println!("{:>9}  {:<10}  {:<13}  NAME", "APPID", "STATUS", "LICENSE");
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
            println!(
                "{:>9}  {:<10}  {:<13}  {:<7}  {}{}",
                g.app_id, status, license, online_col, g.name, branch
            );
        } else {
            println!(
                "{:>9}  {:<10}  {:<13}  {}{}",
                g.app_id, status, license, g.name, branch
            );
        }
    }

    let shared = games.iter().filter(|g| g.is_family_shared).count();
    if shared > 0 {
        println!(
            "\n{} game(s), {} via Family Sharing (not licensed to this account).",
            games.len(),
            shared
        );
    } else {
        println!("\n{} game(s).", games.len());
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
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    println!("Account : {}", data.account_name);
    println!("SteamID : {}", data.steam_id);
    println!("Country : {}", data.country);
    println!(
        "Email   : {} ({})",
        data.email,
        if data.email_validated {
            "validated"
        } else {
            "unvalidated"
        }
    );
    println!("Devices : {}", data.authed_machines);
    println!("VAC bans: {}", data.vac_bans);
    Ok(())
}

async fn cmd_install(
    app_id: u32,
    platform: Option<PlatformArg>,
    restart_steam: bool,
    json: bool,
) -> Result<()> {
    let mut client = authed_client().await?;

    // Note whether this is a DLC so we can refresh Steam's view afterward.
    let is_dlc = client.resolve_dlc_parent(app_id).await.is_some();

    // For a DLC, stop Steam before editing its base appmanifest (Steam overwrites it
    // on exit), then restart it afterward so the running client picks up the change.
    let manage_steam = restart_steam && is_dlc && SteamClient::steam_is_running();
    if manage_steam {
        if !json {
            println!("Stopping Steam ...");
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
                println!("Auto-selected platform: {chosen:?}");
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
            println!("Starting Steam ...");
        }
        SteamClient::start_steam()?;
        steam_restarted = true;
    }

    // A newly installed DLC is invisible to an already-running Steam client until it
    // re-reads the appmanifest (which it does at startup).
    let steam_restart_required = is_dlc && !manage_steam && SteamClient::steam_is_running();
    if steam_restart_required && !json {
        eprintln!();
        eprintln!("Note: the DLC content is installed, but a running Steam client reads DLC state");
        eprintln!("      only at startup. Restart Steam (or re-run with --restart-steam) for it to");
        eprintln!("      be recognized in-game.");
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
        println!("Uninstalled app {app_id}.");
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
        println!("Launching {} ...", game.name);
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
        println!("Finished playing {}.", game.name);
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
            println!("No games are running (none launched via `aurelia play`).");
        } else {
            println!("{:>9}  {:>8}  NAME", "APPID", "PID");
            for g in &running {
                println!("{:>9}  {:>8}  {}", g.app_id, g.pid, g.name);
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
        println!("Stopped {} (app {}).", stopped.name, stopped.app_id);
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
            println!("Stopping Steam ...");
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
            println!("Starting Steam ...");
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

    println!("DLC {app_id} {action} for base game {base}.");
    if enable {
        println!("(Toggles the flag only — run `aurelia install {app_id}` if the content isn't downloaded.)");
    }
    if restart_required {
        eprintln!("Note: Steam is running and reads DLC state only at startup, so this won't apply");
        eprintln!("      until you restart Steam (or re-run with --restart-steam).");
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
        println!("No branches reported.");
    } else {
        for b in branches {
            println!("{b}");
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
        println!("App {app_id} set to branch '{branch}'. Run `aurelia update {app_id}` to apply.");
    }
    Ok(())
}

async fn cmd_info(app_id: u32, json: bool) -> Result<()> {
    // Metadata now comes from the StoreBrowse service over the Steam CM
    // connection (no HTTPS storefront API), so an authenticated session is needed.
    let client = authed_client().await?;

    let details = client
        .fetch_store_apps(&[app_id])
        .await?
        .into_iter()
        .find(|i| i.app_id == app_id)
        .with_context(|| format!("no store information available for app {app_id}"))?;

    // The DLC id list isn't part of StoreBrowse's per-item data; read it from PICS
    // appinfo (`common.dlc`), then resolve the DLC names in a single batched
    // StoreBrowse call.
    let dlc_ids = client
        .get_extended_app_info(app_id)
        .await
        .map(|e| e.dlcs)
        .unwrap_or_default();
    let dlc = resolve_dlc_names_via_store(&client, &dlc_ids).await;

    if json {
        let value = serde_json::json!({
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
            "dlc": dlc.iter().map(|(id, name)| serde_json::json!({"app_id": id, "name": name})).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    // --- Header ---
    let ea = if details.is_early_access { " [Early Access]" } else { "" };
    println!("{}  (app {}){ea}", details.name, details.app_id);
    if !details.app_type.is_empty() {
        println!("Type       : {}", details.app_type);
    }
    if !details.developers.is_empty() {
        println!("Developers : {}", details.developers.join(", "));
    }
    if !details.publishers.is_empty() {
        println!("Publishers : {}", details.publishers.join(", "));
    }
    if !details.franchises.is_empty() {
        println!("Franchises : {}", details.franchises.join(", "));
    }
    if let Some(date) = &details.release_date {
        let suffix = if details.coming_soon { " (coming soon)" } else { "" };
        println!("Released   : {date}{suffix}");
    }
    if let Some(price) = &details.price {
        let discount = if details.discount_pct > 0 {
            format!(" (-{}%)", details.discount_pct)
        } else {
            String::new()
        };
        println!("Price      : {price}{discount}");
    }
    if !details.platforms.is_empty() {
        println!("Platforms  : {}", details.platforms.join(", "));
    }
    if let Some(reviews) = &details.review_summary {
        println!("Reviews    : {reviews}");
    }

    // --- Description ---
    if !details.short_description.is_empty() {
        println!("\nDescription:");
        for line in wrap_text(&details.short_description, 88) {
            println!("  {line}");
        }
    }

    // --- DLC ---
    if !dlc.is_empty() {
        println!("\nDLC ({}):", dlc.len());
        for (id, name) in &dlc {
            let name = name.clone().unwrap_or_else(|| "(name unavailable)".to_string());
            println!("  {id:>9}  {name}");
        }
    }

    Ok(())
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
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    if dlc.is_empty() {
        println!("No DLC for app {app_id}.");
        return Ok(());
    }
    println!("{:>9}  {:<5}  {:<13}  NAME", "APPID", "OWNED", "STATUS");
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
        println!("{id:>9}  {owned:<5}  {status:<13}  {name}");
    }
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
        println!("No depots reported.");
        return Ok(());
    }
    println!("{:>12}  {:>14}  NAME", "DEPOT", "SIZE(bytes)");
    for d in &depots {
        println!("{:>12}  {:>14}  {}", d.id, d.size, d.name);
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
        println!("{}", final_path.display());
    }
    Ok(())
}

async fn cmd_config_show(_json: bool) -> Result<()> {
    // The launcher configuration is structured data; it always renders as JSON.
    let config = load_launcher_config().await.unwrap_or_default();
    println!("{}", serde_json::to_string_pretty(&config)?);
    Ok(())
}

fn cmd_config_protons(json: bool) -> Result<()> {
    let (steam, custom) = scan_proton_runtimes();
    if json {
        print_json(&serde_json::json!({ "steam": steam, "custom": custom }));
        return Ok(());
    }
    println!("Steam runtimes:");
    for s in &steam {
        println!("  {s}");
    }
    if !custom.is_empty() {
        println!("Custom (compatibilitytools.d):");
        for c in &custom {
            println!("  {c}");
        }
    }
    Ok(())
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
    while let Some(p) = rx.recv().await {
        match p.state {
            DownloadProgressState::Queued => {
                if json {
                    emit_progress_json("queued", &p, &mut last);
                } else {
                    println!("Queued ...");
                }
            }
            DownloadProgressState::Downloading => {
                if json {
                    emit_progress_json("downloading", &p, &mut last);
                } else {
                    print_progress("Downloading", &p);
                }
            }
            DownloadProgressState::Verifying => {
                if json {
                    emit_progress_json("verifying", &p, &mut last);
                } else {
                    print_progress("Verifying", &p);
                }
            }
            DownloadProgressState::Completed => {
                // The caller emits the terminal result object; nothing more here.
                if !json {
                    println!("\nDone.");
                }
                return Ok(());
            }
            DownloadProgressState::Failed => {
                if !json {
                    println!();
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
/// Reports both the whole-app progress (`percent`) and the current depot's progress
/// (`depot_percent`).
fn emit_progress_json(state: &str, p: &DownloadProgress, last: &mut Option<(u8, u64, u64)>) {
    // Cheap discriminator for the state so we can dedupe (state, overall, depot).
    let state_key = match state {
        "queued" => 0u8,
        "downloading" => 1,
        "verifying" => 2,
        _ => 3,
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
        "file": p.current_file,
    });
    // Compact single line so the whole --json stream is valid NDJSON.
    if let Ok(s) = serde_json::to_string(&value) {
        println!("{s}");
    }
}

fn print_progress(phase: &str, p: &DownloadProgress) {
    let overall = percent_of(p.bytes_downloaded, p.total_bytes);
    if p.depot_id != 0 {
        let depot = percent_of(p.depot_bytes_downloaded, p.depot_total_bytes);
        print!(
            "\r{phase}: {overall:5.1}% overall  {}/{} bytes  | depot {}: {depot:5.1}%   ",
            p.bytes_downloaded, p.total_bytes, p.depot_id
        );
    } else {
        print!(
            "\r{phase}: {overall:5.1}%  {}/{} bytes  {}",
            p.bytes_downloaded, p.total_bytes, p.current_file
        );
    }
    let _ = std::io::stdout().flush();
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
    eprint!("{prompt}");
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
