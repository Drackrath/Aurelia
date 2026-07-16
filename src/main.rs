use anyhow::{bail, Result};
use clap::Parser;

#[macro_use]
#[path = "core/output.rs"]
mod output;
mod daemon;
#[path = "compat/proc_admin.rs"]
mod proc_admin;
#[path = "web/steam_urls.rs"]
mod steam_urls;

mod cli;
use crate::cli::*;

mod commands;
use crate::commands::*;

/// ASCII-art banner shown when `aurelia` is run with no subcommand.
const BANNER: &str = include_str!("../assets/asciiart_banner.txt");

/// Print the banner followed by the top-level long help. Used for a bare
/// `aurelia` invocation so the user sees the logo and then every command.
fn print_banner_and_help() {
    use clap::CommandFactory;
    cli_println!("{BANNER}\n");
    cli_print!("{}", Cli::command().render_long_help());
}

fn main() {
    // Translate any configured network proxy into the standard proxy environment
    // variables *before* the async runtime or worker threads start, so every reqwest
    // client in the process (including those in vendored crates) routes through it.
    // Doing this while the process is still single-threaded keeps the env mutation
    // sound. A spawned `daemon` subprocess re-runs this through its own `main`.
    aurelia::core::net::install_proxy_env(&aurelia::core::config::load_proxy_config_blocking());

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
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            use clap::error::ErrorKind;
            // Bare `aurelia` (no subcommand): greet with the banner, then the full
            // help — instead of clap's terse "requires a subcommand" error.
            if matches!(
                e.kind(),
                ErrorKind::MissingSubcommand
                    | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            ) {
                print_banner_and_help();
                std::process::exit(0);
            }
            // Everything else (genuine parse errors, --help, --version) keeps clap's
            // default rendering and exit codes.
            e.exit();
        }
    };

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
    aurelia::core::config::ensure_config_dirs().await?;

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
            collection,
            online,
            check_updates,
        } => cmd_list(installed, search, collection, online, check_updates, json).await,
        Command::Account => cmd_account(json).await,
        Command::Collections { command } => match command {
            CollectionsCommand::List => cmd_collections_list(json).await,
            CollectionsCommand::Show { name } => cmd_collections_show(name, json).await,
            CollectionsCommand::Create { name } => cmd_collections_create(name, json).await,
            CollectionsCommand::Delete { name } => cmd_collections_delete(name, json).await,
            CollectionsCommand::Rename { name, new_name } => {
                cmd_collections_rename(name, new_name, json).await
            }
            CollectionsCommand::Add { name, app_ids } => {
                cmd_collections_add(name, app_ids, json).await
            }
            CollectionsCommand::Remove { name, app_ids } => {
                cmd_collections_remove(name, app_ids, json).await
            }
            CollectionsCommand::Pull => cmd_collections_pull(json).await,
            CollectionsCommand::Push { yes } => cmd_collections_push(yes, json).await,
            CollectionsCommand::Sync { yes } => cmd_collections_sync(yes, json).await,
        },
        Command::Friends { command } => match command {
            None | Some(FriendsCommand::List) => cmd_friends(json).await,
            Some(FriendsCommand::Search { query }) => cmd_friends_search(query, json).await,
            Some(FriendsCommand::Add { query }) => cmd_friends_add(query, json).await,
            Some(FriendsCommand::Remove { steamid }) => cmd_friends_remove(steamid, json).await,
        },
        Command::Chat { command } => match command {
            ChatCommand::Send { steamid, message } => {
                cmd_chat_send(steamid, message.join(" "), json).await
            }
            ChatCommand::History { steamid, count } => {
                cmd_chat_history(steamid, count, json).await
            }
            ChatCommand::Open { steamid } => cmd_chat_open(steamid, json).await,
        },
        Command::Inventory { app_id, context } => cmd_inventory(app_id, context, json).await,
        Command::Wallet => cmd_wallet(json).await,
        Command::Market { command } => match command {
            MarketCommand::Price {
                app_id,
                name,
                currency,
            } => cmd_market_price(app_id, name, currency, json).await,
            MarketCommand::Search {
                query,
                app_id,
                count,
            } => cmd_market_search(query, app_id, count, json).await,
            MarketCommand::Listings => cmd_market_listings(json).await,
        },
        Command::Install(args) => match args.action {
            Some(InstallAction::List) => cmd_install_list(json).await,
            Some(InstallAction::Stop { app_id }) => cmd_install_stop(app_id, json).await,
            None => {
                let Some(app_id) = args.app_id else {
                    bail!("an app id is required, e.g. `aurelia install 945360`");
                };
                cmd_install(
                    app_id,
                    args.platform,
                    args.restart_steam,
                    args.dry_run,
                    args.library,
                    json,
                )
                .await
            }
        },
        Command::Libraries => cmd_libraries(json).await,
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
        Command::Update { app_id, force } => match app_id {
            Some(id) => cmd_update(id, force, json).await,
            None => cmd_check_updates(json).await,
        },
        Command::Manifests { app_id, depot } => cmd_manifests(app_id, depot, json).await,
        Command::Downgrade(args) => cmd_downgrade(args, json).await,
        Command::Pin { app_id } => cmd_pin(app_id, json).await,
        Command::Unpin { app_id } => cmd_unpin(app_id, json).await,
        Command::Play {
            app_id,
            proton,
            windows,
            native_engine,
            umu,
            script,
            no_script,
            steam,
            noupdate,
        } => cmd_play(app_id, proton, windows, native_engine, umu, script, no_script, steam, noupdate, json).await,
        Command::Running => cmd_running(json),
        Command::Stop { app_id, force } => cmd_stop(app_id, force, json).await,
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
            lang,
        } => cmd_info(app_ids, extended, no_cache, lang, json).await,
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
            ConfigCommand::Protons => cmd_config_protons(json).await,
            ConfigCommand::Presence { mode } => cmd_config_presence(mode, json).await,
            ConfigCommand::Language { lang } => cmd_config_language(lang, json).await,
            ConfigCommand::Proxy { url, no_proxy, clear } => {
                cmd_config_proxy(url, no_proxy, clear, json).await
            }
            ConfigCommand::Game {
                app_id,
                proton,
                clear_proton,
                platform,
                native_engine,
                no_native_engine,
                umu,
                no_umu,
                launch_script,
                no_launch_script,
            } => cmd_config_game(app_id, proton, clear_proton, platform, native_engine, no_native_engine, umu, no_umu, launch_script, no_launch_script, json).await,
        },
        Command::Cloud { command } => match command {
            CloudCommand::Sync {
                app_id,
                up,
                down,
                path,
                resolve,
            } => cmd_cloud_sync(app_id, up, down, path, resolve, json).await,
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
        Command::Proton { command } => match command {
            ProtonCommand::List { installed } => cmd_proton_list(installed, json).await,
            ProtonCommand::Install { version } => cmd_proton_install(version, json).await,
            ProtonCommand::Uninstall { version } => cmd_proton_uninstall(version, json).await,
            ProtonCommand::Default { version } => cmd_proton_default(version, json).await,
        },
        Command::SteamRuntime { command } => match command {
            SteamRuntimeCommand::Install => cmd_steam_runtime_install(json).await,
            SteamRuntimeCommand::Repair => cmd_steam_runtime_repair(json).await,
            SteamRuntimeCommand::Status => cmd_steam_runtime_status(json).await,
        },
        Command::Luxtorpeda { command } => match command {
            LuxtorpedaCommand::Enable => cmd_luxtorpeda_toggle(true, json).await,
            LuxtorpedaCommand::Disable => cmd_luxtorpeda_toggle(false, json).await,
            LuxtorpedaCommand::Install | LuxtorpedaCommand::Update => {
                cmd_luxtorpeda_install(json).await
            }
            LuxtorpedaCommand::Status => cmd_luxtorpeda_status(json).await,
            LuxtorpedaCommand::Path { path, clear } => cmd_luxtorpeda_path(path, clear, json).await,
            LuxtorpedaCommand::Uninstall => cmd_luxtorpeda_uninstall(json).await,
        },
        Command::Umu { command } => match command {
            UmuCommand::Enable => cmd_umu_toggle(true, json).await,
            UmuCommand::Disable => cmd_umu_toggle(false, json).await,
            UmuCommand::Install | UmuCommand::Update => cmd_umu_install(json).await,
            UmuCommand::Status => cmd_umu_status(json).await,
            UmuCommand::Path { path, clear } => cmd_umu_path(path, clear, json).await,
            UmuCommand::Uninstall => cmd_umu_uninstall(json).await,
        },
        Command::Scripts { command } => match command {
            ScriptsCommand::Dir => cmd_scripts_dir(json).await,
            ScriptsCommand::List => cmd_scripts_list(json).await,
            ScriptsCommand::New { app_id, force } => cmd_scripts_new(app_id, force, json).await,
            ScriptsCommand::Show { app_id } => cmd_scripts_show(app_id, json).await,
            ScriptsCommand::Remove { app_id } => cmd_scripts_remove(app_id, json).await,
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

/// Discover Proton/Wine runtimes under the user's Steam directories.
#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
