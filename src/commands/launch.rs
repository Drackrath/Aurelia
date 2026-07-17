//! `launch` command handlers.

use crate::proc_admin;

use crate::commands::common::*;

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use anyhow::{bail, Context, Result};
use aurelia::core::config::load_launcher_config;
use aurelia::core::config::load_user_configs;
use aurelia::core::models::DownloadState;
use aurelia::steam_client::SteamClient;

pub(crate) async fn cmd_play(
    app_id: u32,
    proton: Option<String>,
    windows: bool,
    native_engine: bool,
    umu: bool,
    script: Option<PathBuf>,
    no_script: bool,
    steam: bool,
    noupdate: bool,
    json: bool,
) -> Result<()> {
    if native_engine && !cfg!(target_os = "linux") {
        anyhow::bail!("--native-engine (luxtorpeda) is only available on Linux");
    }
    if umu && !cfg!(target_os = "linux") {
        anyhow::bail!("--umu (umu-launcher) is only available on Linux");
    }
    // `--windows` runs the executable directly with no Proton/Wine layer, which
    // can only work on a Windows host. On Linux a Windows PE can't be exec'd
    // natively (it fails with "Permission denied"), so reject it up front and
    // point the user at the default Proton path.
    if windows && !cfg!(target_os = "windows") {
        anyhow::bail!(
            "--windows runs the game with no Proton/Wine layer and only works on a Windows host; \
             on Linux omit it (optionally pass --proton <runner>) to run the game through Proton"
        );
    }
    let mut client = authed_client().await?;
    let mut game = find_game(&mut client, app_id).await?;

    // A game whose only local copy lives in the in-Wine Steam runtime's own library
    // (installed through the in-Wine Steam itself — the only route for Family-Shared
    // titles Aurelia can't download) can't be run through the Proton pipeline: its
    // Steamworks handshake needs the full context only the running Steam client sets
    // up. Hand it to the in-Wine Steam, exactly as launching it from that Steam's GUI
    // does. Its updates are the in-Wine Steam's responsibility, so skip Aurelia's
    // pre-launch update flow (the game isn't in Aurelia's library to update anyway).
    if game.from_windows_steam {
        let launcher_config = load_launcher_config().await?;
        let install_path = game.install_path.clone().unwrap_or_default();
        if !json {
            cli_println!(
                "⚠ {} lives in the in-Wine Steam runtime's own library — launching it \
                 through the in-Wine Steam (Wine). Aurelia's Proton/DXVK settings and \
                 session tracking don't apply to this launch.",
                game.name
            );
            cli_println!("Launching {} via the in-Wine Steam ...", game.name);
        }
        aurelia::launch::launch_game_via_master_steam(
            &launcher_config,
            app_id,
            std::path::Path::new(&install_path),
        )
        .await
        .with_context(|| format!("failed to launch {} via the in-Wine Steam", game.name))?;
        if json {
            print_json(&serde_json::json!({
                "app_id": app_id,
                "name": game.name,
                "status": "finished",
                "runtime": "in-wine-steam",
            }));
        } else {
            cli_println!("Finished playing {}.", game.name);
        }
        return Ok(());
    }

    // Family-Shared games are pinned to the owner's current build — a stale
    // install simply won't launch — so for them the pre-launch update is
    // *required*, not best-effort: any failure aborts the launch (running the old
    // build would just fail cryptically) and `--noupdate` is ignored.
    let update_required = game.is_installed && game.is_family_shared;

    // Pull the latest build before launching. For owned games this is best-effort
    // (a failed check/download must not stop the user playing what's installed);
    // for Family-Shared games it is mandatory (see above).
    if game.is_installed && (!noupdate || update_required) {
        if let Err(e) = client.check_for_updates(std::slice::from_mut(&mut game)).await {
            if update_required {
                bail!(
                    "could not verify the latest build for Family-Shared game {} ({e:#}); \
                     it must be up to date to launch",
                    game.name
                );
            }
            tracing::warn!("pre-launch update check failed ({e:#}); launching current version");
        }
        if game.update_available {
            if !json {
                let kind = if update_required { "Required update" } else { "Update available" };
                cli_println!("{kind} for {} — installing ...", game.name);
            }
            let state = Arc::new(RwLock::new(DownloadState::default()));
            let update_result = match client.update_game(app_id, state).await {
                Ok(rx) => drive_progress(rx, json).await,
                Err(e) => Err(e),
            };
            if let Err(e) = update_result {
                if update_required {
                    bail!(
                        "required update for Family-Shared game {} failed ({e:#}); \
                         it can't launch until updated",
                        game.name
                    );
                }
                tracing::warn!("pre-launch update failed ({e:#}); launching current version");
            }
        }
    }

    // Proton/Wine is Linux-only; on Windows we always run the game natively.
    let force_windows = windows || cfg!(target_os = "windows");

    let launcher_config = load_launcher_config().await?;
    let game_cfg = launcher_config.game_configs.get(&app_id);
    let forced_proton = game_cfg.and_then(|c| c.forced_proton_version.clone());
    let prefers_windows =
        game_cfg.and_then(|c| c.platform_preference.as_deref()) == Some("windows");

    // Resolution order: explicit `--proton` flag → the game's stored version →
    // (when the game targets Windows) the global default. None means run natively.
    let proton_path = proton
        .or(forced_proton)
        .or_else(|| prefers_windows.then(|| launcher_config.proton_version.clone()));

    let user_configs = load_user_configs().await?;
    let user_config = user_configs.get(&app_id);

    if !json {
        cli_println!("Launching {} ...", game.name);
    }
    client
        .play_game(&game, proton_path.as_deref(), user_config, force_windows, native_engine, umu, script, no_script, steam)
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

/// `aurelia running`: list the games Aurelia currently has running (stale records
/// whose process has exited are pruned).
pub(crate) fn cmd_running(json: bool) -> Result<()> {
    let running = aurelia::compat::running::list_active();
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
    Ok(())
}

pub(crate) async fn cmd_stop(app_id: Option<u32>, force: bool, json: bool) -> Result<()> {
    // No app id: report what Aurelia currently tracks as running.
    let Some(app_id) = app_id else {
        return cmd_running(json);
    };

    let stopped = SteamClient::stop_game(app_id, force)
        .with_context(|| format!("failed to stop app {app_id}"))?;
    if json {
        print_json(&serde_json::json!({
            "app_id": stopped.app_id,
            "name": stopped.name,
            "status": "stopped",
            "forced": force,
        }));
    } else {
        let how = if force { " (forced)" } else { "" };
        cli_println!("Stopped {} (app {}){}.", stopped.name, stopped.app_id, how);
    }
    Ok(())
}

/// `aurelia kill`: terminate every running aurelia process (daemon and otherwise),
/// except the current one.
pub(crate) fn cmd_kill(json: bool) -> Result<()> {
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
pub(crate) fn cmd_daemon_stop(pid: Option<u32>, json: bool) -> Result<()> {
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
pub(crate) fn cmd_daemon_list(json: bool) -> Result<()> {
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
