//! Game prefix preparation and background Steam.

use super::steam_mode::SteamMode;
use super::{ReadinessGrace, SteamLayout, reclassify_after_grace, resolve_proton_required};
use crate::launch::error::{LaunchError, LaunchErrorKind};
use crate::launch::runners::LaunchContext;
use crate::steam_client::SteamClient;
use anyhow::anyhow;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Create prefixes and start background Steam.
pub(super) async fn prepare_prefix(ctx: &LaunchContext) -> std::result::Result<(), LaunchError> {
    let SteamLayout {
        library_root,
        steam_mode,
        steam_mode_source: runtime_source,
        steam_prefix_mode,
        compat_data_path,
        game_prefix: effective_game_prefix,
    } = SteamLayout::derive(ctx);
    let use_steam_runtime = steam_mode == SteamMode::InWineRuntime;
    std::fs::create_dir_all(&effective_game_prefix).map_err(|e| {
        LaunchError::new(
            LaunchErrorKind::Permission,
            format!("failed creating {}", effective_game_prefix.display()),
        )
        .with_source(anyhow!(e))
    })?;

    // Proton's `pfx.lock` needs existing compatdata.
    std::fs::create_dir_all(&compat_data_path).map_err(|e| {
        LaunchError::new(
            LaunchErrorKind::Permission,
            format!("failed creating {}", compat_data_path.display()),
        )
        .with_source(anyhow!(e))
    })?;

    tracing::info!("Effective game prefix: {}", effective_game_prefix.display());
    tracing::info!(
        "Shared steam compatibility data enabled: {}",
        ctx.launcher_config.use_shared_compat_data
    );
    tracing::info!("Steam Runtime Prefix Mode: {:?}", steam_prefix_mode);

    if use_steam_runtime {
        prepare_steam_runtime(
            ctx,
            &library_root,
            steam_prefix_mode,
            runtime_source,
            &effective_game_prefix,
        )
        .await?;
    }

    // Keep any shipped steam_appid.txt
    let game = ctx.launch_info.command(ctx.install_dir()?);

    crate::core::fsutil::ensure_steam_appid_logged(&game.cwd, ctx.app.app_id);

    Ok(())
}

/// Start the in-Wine Steam runtime.
async fn prepare_steam_runtime(
    ctx: &LaunchContext,
    library_root: &Path,
    steam_prefix_mode: crate::core::models::SteamPrefixMode,
    runtime_source: &'static str,
    effective_game_prefix: &Path,
) -> std::result::Result<(), LaunchError> {
    let steam_cfg = crate::core::steam_host::get_master_steam_config();
    tracing::info!("Unified Master Steam resolution (Game Launch):");
    tracing::info!("  - Root Dir: {}", steam_cfg.root_dir.display());
    tracing::info!("  - Wine Prefix: {}", steam_cfg.wine_prefix.display());
    tracing::info!("  - Layout Kind: {}", steam_cfg.layout_kind);

    let master_steam_dir = match &steam_cfg.steam_exe {
        Some(exe) => exe.parent().unwrap().to_path_buf(),
        None => {
            return Err(LaunchError::new(
                LaunchErrorKind::Environment,
                format!(
                    "use_steam_runtime is enabled but steam.exe was not found in {}.\n\
                     Go to Settings → 'Install / Manage Windows Steam Runtime' first.",
                    steam_cfg.wine_prefix.display()
                ),
            )
            .with_context("master_prefix", steam_cfg.wine_prefix.to_string_lossy()));
        }
    };

    tracing::info!(
        "  - Steam Exe: {}",
        steam_cfg.steam_exe.as_ref().unwrap().display()
    );

    let (prefix_steam_dir, steam_wineprefix) = match steam_prefix_mode {
        crate::core::models::SteamPrefixMode::Shared => {
            (master_steam_dir.clone(), steam_cfg.wine_prefix.clone())
        }
        crate::core::models::SteamPrefixMode::PerGame => (
            deploy_per_game_steam(&master_steam_dir, effective_game_prefix),
            effective_game_prefix.to_path_buf(),
        ),
    };

    tracing::debug!("Runtime Steam dir : {}", prefix_steam_dir.display());
    tracing::debug!("Runtime WINEPREFIX : {}", steam_wineprefix.display());

    SteamClient::write_headless_steam_cfg(&prefix_steam_dir);

    let steam_args = background_steam_args(ctx);

    let steam_running = SteamClient::is_steam_running_in_prefix(&steam_wineprefix);

    ctx.with_verification(|v| {
        v.steam_running_before_launch = steam_running;
        v.effective_game_wineprefix = Some(effective_game_prefix.to_string_lossy().to_string());
        v.effective_steam_wineprefix = Some(steam_wineprefix.to_string_lossy().to_string());
        v.per_game_prefix_requested =
            steam_prefix_mode == crate::core::models::SteamPrefixMode::PerGame;
        v.per_game_prefix_honored = effective_game_prefix == steam_wineprefix;
        v.steam_runtime_policy = format!(
            "{:?}",
            ctx.user_config
                .as_ref()
                .map(|c| &c.steam_runtime_policy)
                .unwrap_or(&crate::core::models::SteamRuntimePolicy::Auto)
        );
        v.steam_runtime_source = runtime_source.to_string();
        v.windows_steam_discovery_enabled = ctx.launcher_config.windows_steam_discovery_enabled;
    });

    if steam_running {
        tracing::info!("Steam already running in prefix — skipping spawn");
    } else {
        start_background_steam(
            ctx,
            library_root,
            &prefix_steam_dir,
            &steam_wineprefix,
            &steam_args,
        )
        .await?;
    }
    Ok(())
}

/// Link Steam runtime files into a per-game prefix.
fn deploy_per_game_steam(master_steam_dir: &Path, effective_game_prefix: &Path) -> PathBuf {
    let target_steam_dir = effective_game_prefix.join("drive_c/Program Files (x86)/Steam");

    tracing::info!(
        "Deploying required Steam runtime files to {}",
        target_steam_dir.display()
    );
    let _ = std::fs::create_dir_all(&target_steam_dir);

    let required_files = [
        "steam.exe",
        "steamclient.dll",
        "steamclient64.dll",
        "tier0_s.dll",
        "tier0_s64.dll",
        "vstdlib_s.dll",
        "vstdlib_s64.dll",
    ];

    for file in required_files {
        let src = master_steam_dir.join(file);
        let dst = target_steam_dir.join(file);
        if src.exists() && !dst.exists() {
            #[cfg(unix)]
            {
                if let Err(e) = std::os::unix::fs::symlink(&src, &dst) {
                    tracing::warn!("Symlink failed for {}, falling back to copy: {}", file, e);
                    let _ = std::fs::copy(&src, &dst);
                }
            }
            #[cfg(not(unix))]
            {
                let _ = std::fs::copy(&src, &dst);
            }
        }
    }

    // Also symlink required subdirectories
    let required_dirs = ["bin", "public"];
    for dir in required_dirs {
        let src = master_steam_dir.join(dir);
        let dst = target_steam_dir.join(dir);
        if src.exists() && !dst.exists() {
            #[cfg(unix)]
            {
                if let Err(e) = std::os::unix::fs::symlink(&src, &dst) {
                    tracing::warn!("Symlink failed for {}, falling back to copy: {}", dir, e);
                    let _ = crate::core::fsutil::copy_dir_all(&src, &dst);
                }
            }
            #[cfg(not(unix))]
            {
                let _ = crate::core::fsutil::copy_dir_all(&src, &dst);
            }
        }
    }

    target_steam_dir
}

/// Steam client flags for background Steam.
fn background_steam_args(ctx: &LaunchContext) -> Vec<String> {
    let slc = ctx
        .user_config
        .as_ref()
        .map(|c| c.steam_launch_config.clone())
        .unwrap_or_default();

    let mut steam_args = vec![
        "-silent".to_string(),
        "-tcp".to_string(),
        "-noverifyfiles".to_string(),
        "-noreactlogin".to_string(),
        "-cef-disable-gpu".to_string(),
        "-cef-disable-sandbox".to_string(),
    ];

    if slc.no_friends_ui {
        steam_args.push("-nofriendsui".to_string());
    }
    if slc.no_chat_ui {
        steam_args.push("-nochatui".to_string());
    }
    if slc.no_browser {
        steam_args.push("-no-browser".to_string());
    }
    if slc.no_overlay {
        steam_args.push("-disable-overlay".to_string());
    }
    if slc.no_vr {
        steam_args.push("-noopenvr".to_string());
    }
    if slc.big_picture {
        steam_args.push("-bigpicture".to_string());
    }
    steam_args
}

/// Spawn background Steam, wait until ready.
async fn start_background_steam(
    ctx: &LaunchContext,
    library_root: &Path,
    prefix_steam_dir: &Path,
    steam_wineprefix: &Path,
    steam_args: &[String],
) -> std::result::Result<(), LaunchError> {
    // Background Steam is hosted on a DEDICATED Steam-runtime runner —
    // NOT the game runner's `proton run` wrapper. It must run under a
    // bare wine so `steam.exe` starts as a plain Windows process.
    let mut steam_cmd = resolve_background_steam_command(ctx, library_root)?;
    // Background Steam's CEF UI needs the runner's dxvk/vkd3d PE
    // libs in the prefix — a bare-wine prefix misses them and the
    // webhelper GPU process crash-loops (Steam then misreports a
    // broken installation).
    crate::core::runner::ensure_steam_runtime_prefix_libs(
        &PathBuf::from(steam_cmd.get_program()),
        steam_wineprefix,
    );
    steam_cmd.current_dir(prefix_steam_dir);
    steam_cmd
        .arg("C:\\Program Files (x86)\\Steam\\steam.exe")
        .args(steam_args);
    steam_cmd
        .env("WINEPREFIX", steam_wineprefix)
        .env(
            "WINEDLLOVERRIDES",
            "vstdlib_s=n,b;tier0_s=n,b;steamclient=n,b;steamclient64=n,b;\
                 steam_api=n,b;steam_api64=n,b;lsteamclient=;\
                 GameOverlayRenderer=n;GameOverlayRenderer64=n",
        )
        .env("WINEPATH", "C:\\Program Files (x86)\\Steam")
        .env("STEAM_DISABLE_BROWSER", "1")
        .env("STEAM_NO_BROWSER", "1")
        .env("STEAMCMD", "1") // tells Steam it's running as a cmd tool
        .stdout(std::process::Stdio::null()) // silence CEF log spam
        .stderr(std::process::Stdio::null());

    tracing::debug!(
        program = ?steam_cmd.get_program(),
        args = ?steam_cmd.get_args().collect::<Vec<_>>(),
        "spawning background Steam",
    );

    // Record Steam runtime diagnostics
    ctx.with_verification(|v| {
        v.steam_runtime_exe = Some(steam_cmd.get_program().to_string_lossy().to_string());
        v.steam_runtime_args = steam_cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        v.steam_runtime_milestone = "steam_process_spawn_requested".to_string();
        v.steam_auto_start_attempted = true;
    });

    let start_time = std::time::Instant::now();
    let mut steam_process = steam_cmd.spawn().map_err(|e| {
        LaunchError::new(LaunchErrorKind::Process, "Failed to spawn background Steam")
            .with_source(anyhow!(e))
    })?;

    ctx.with_verification(|v| {
        v.steam_runtime_milestone = "steam_process_spawned".to_string();
    });

    let ready = wait_for_steam_ready(
        ctx,
        &mut steam_process,
        start_time,
        prefix_steam_dir,
        steam_wineprefix,
    )
    .await;

    if !ready {
        ctx.with_verification(|v| {
            v.steam_auto_start_failed = true;
        });
        return Err(LaunchError::new(
            LaunchErrorKind::Process,
            "Background Steam crashed before the game could start",
        ));
    }
    Ok(())
}

/// Poll ready signals, then a liveness grace.
async fn wait_for_steam_ready(
    ctx: &LaunchContext,
    steam_process: &mut std::process::Child,
    start_time: std::time::Instant,
    prefix_steam_dir: &Path,
    steam_wineprefix: &Path,
) -> bool {
    let readiness_timeout = 8;
    tracing::info!(
        "Waiting for Steam to initialise (max {}s)...",
        readiness_timeout
    );

    let steam_pid_path = prefix_steam_dir.join("steam.pid");
    let steam_pipe = steam_wineprefix.join("drive_c/windows/temp/.steampath");
    let steam_config_vdf = prefix_steam_dir.join("config/config.vdf");
    let steam_logs_dir = prefix_steam_dir.join("logs");

    // Which readiness heuristic fired, if any. We record it and
    // break the poll loop instead of returning `true` immediately
    // so a short liveness grace window below can still reclassify
    // a signal-then-crash as an early exit rather than "ready".
    let mut ready_signal: Option<String> = None;
    for i in 0..readiness_timeout {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        // Crash detection — bail immediately
        if let Ok(Some(status)) = steam_process.try_wait() {
            tracing::error!("Background Steam exited after {}s with: {}", i + 1, status);
            ctx.with_verification(|v| {
                v.steam_runtime_exit_code = status.code();
                v.steam_runtime_lifetime_ms = Some(start_time.elapsed().as_millis() as u64);
                v.steam_runtime_milestone = "steam_process_exited_early".to_string();
            });
            return false;
        }

        // Signal 1: pid file (some Wine/Steam combos do write this)
        if steam_pid_path.exists() {
            tracing::info!("Steam ready after {}s (steam.pid found)", i + 1);
            ready_signal = Some(format!("steam.pid after {}s", i + 1));
            break;
        }

        // Signal 2: .steampath in temp (Proton-style)
        if steam_pipe.exists() {
            tracing::info!("Steam ready after {}s (.steampath found)", i + 1);
            ready_signal = Some(format!(".steampath after {}s", i + 1));
            break;
        }

        // Signal 3: config.vdf written — Steam has finished early init
        if steam_config_vdf.exists() {
            tracing::info!("Steam ready after {}s (config.vdf found)", i + 1);
            ready_signal = Some(format!("config.vdf after {}s", i + 1));
            break;
        }

        // Signal 4: logs dir has multiple entries — Steam's subsystems are running
        let log_count = std::fs::read_dir(&steam_logs_dir)
            .map(|d| d.count())
            .unwrap_or(0);
        if log_count >= 2 {
            tracing::info!(
                "Steam ready after {}s ({} log files found)",
                i + 1,
                log_count
            );
            ctx.with_verification(|v| {
                v.steam_runtime_milestone = "steam_ready_signal_observed".to_string();
            });
            ready_signal = Some(format!("{} log files after {}s", log_count, i + 1));
            break;
        }

        tracing::debug!("Waiting... {}s", i + 1);
    }

    // Liveness grace window: after a readiness heuristic fires, a
    // healthy Steam keeps running while a crashing one exits within
    // a second or two. Wait a short grace period and re-check the
    // process; if it has exited, this was a crash — not readiness —
    // so classify it as `steam_process_exited_early` (consumed by
    // pipeline.rs) exactly like the top-of-loop crash detection.
    if let Some(reason) = ready_signal {
        const READINESS_GRACE_SECS: u64 = 2;
        tokio::time::sleep(std::time::Duration::from_secs(READINESS_GRACE_SECS)).await;
        match reclassify_after_grace(steam_process) {
            ReadinessGrace::ExitedEarly { code } => {
                tracing::error!(
                    "Background Steam signalled ready ({}) but exited within {}s grace (code {:?})",
                    reason,
                    READINESS_GRACE_SECS,
                    code
                );
                ctx.with_verification(|v| {
                    v.steam_runtime_exit_code = code;
                    v.steam_runtime_lifetime_ms = Some(start_time.elapsed().as_millis() as u64);
                    v.steam_runtime_milestone = "steam_process_exited_early".to_string();
                });
                return false;
            }
            // Still alive after the grace window — genuinely ready.
            ReadinessGrace::Ready => return true,
        }
    }

    tracing::warn!(
        "Steam did not signal ready after {}s, launching game anyway",
        readiness_timeout
    );
    ctx.with_verification(|v| {
        v.steam_runtime_milestone = "steam_ready_timeout".to_string();
    });
    true
}

/// Resolve the command used to host the background Steam client under a BARE wine.
///
/// Background Steam must never be launched through the game runner's `proton run`
/// protonfixes wrapper — it needs a plain wine so `steam.exe` runs as an ordinary
/// Windows process. Resolution order:
///   1. The explicitly configured `steam_runtime_runner`, resolved to its bare wine
///      binary — a Proton tree is accepted and its bundled `files/bin/wine64` is used
///      (NOT `proton run`), exactly as `steam-runtime install` resolves it.
///   2. A wine-tkg / plain-Wine runtime (based on the game runner when it already is
///      one).
///   3. If the only available runtime is a Proton tree, its bundled bare
///      `files/bin/wine64` directly (NOT `proton run`).
///
/// If none of these yield a usable bare wine, a clear [`LaunchError`] is returned —
/// it NEVER silently falls back to hosting Steam on the game's `proton run` runner.
fn resolve_background_steam_command(
    ctx: &LaunchContext,
    library_root: &Path,
) -> std::result::Result<Command, LaunchError> {
    // 1. Prefer the explicitly configured Steam-runtime runner. Resolve it to the bare
    //    wine binary the same way `steam-runtime install` does: this accepts a Proton
    //    tree and uses the bare wine bundled inside it (`files/bin/wine64`), rather than
    //    rejecting Proton or invoking the `proton run` wrapper.
    let configured = &ctx.launcher_config.steam_runtime_runner;
    if !configured.as_os_str().is_empty() {
        let name = configured.to_string_lossy();
        let bare_wine = crate::core::runner::resolve_steam_runtime_wine(&name, library_root)
            .map_err(|e| {
                LaunchError::new(
                    LaunchErrorKind::Runner,
                    format!(
                        "configured steam_runtime_runner '{name}' could not be resolved to a bare wine for background Steam"
                    ),
                )
                .with_source(e)
            })?;
        tracing::info!(
            "Background Steam runner: configured steam_runtime_runner '{name}' -> bare wine {}",
            bare_wine.display()
        );
        return Ok(Command::new(bare_wine));
    }

    // 2/3. Otherwise resolve a bare-wine runtime from what is available. The game
    // runner is the runtime we know exists for this launch, so classify it.
    let proton = resolve_proton_required(ctx)?;
    let game_runner = crate::core::runner::resolve_runner(proton, library_root);
    match crate::core::runner::classify_runner(&game_runner) {
        crate::core::runner::RunnerKind::WineTkg | crate::core::runner::RunnerKind::PlainWine => {
            tracing::info!(
                "Background Steam runner: bare wine tree {}",
                game_runner.display()
            );
            crate::core::runner::build_runner_command(&game_runner).map_err(|e| {
                LaunchError::new(
                    LaunchErrorKind::Runner,
                    format!(
                        "Invalid Compatibility Layer path: {}",
                        game_runner.display()
                    ),
                )
                .with_source(e)
            })
        }
        crate::core::runner::RunnerKind::Proton => {
            // Only a Proton tree is available — use its bundled bare wine directly,
            // NOT `proton run` (the protonfixes wrapper is wrong for background Steam).
            let bare = crate::core::runner::proton_bundled_bare_wine(&game_runner).ok_or_else(|| {
                LaunchError::new(
                    LaunchErrorKind::Environment,
                    format!(
                        "the only available runtime is a Proton tree ({}) but its bundled bare wine \
                         (files/bin/wine64) could not be found; background Steam needs a bare wine",
                        game_runner.display()
                    ),
                )
            })?;
            tracing::info!(
                "Background Steam runner: Proton-bundled bare wine {}",
                bare.display()
            );
            Ok(Command::new(bare))
        }
        crate::core::runner::RunnerKind::Unknown => Err(LaunchError::new(
            LaunchErrorKind::Runner,
            format!(
                "no suitable Steam-runtime runner found for background Steam. Configure \
                 `steam_runtime_runner` with a wine-tkg or plain-Wine build (the game runner '{}' \
                 is not a usable bare-wine runtime)",
                game_runner.display()
            ),
        )),
    }
}
