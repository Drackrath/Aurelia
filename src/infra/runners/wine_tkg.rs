use std::collections::HashMap;
use anyhow::anyhow;
use std::path::{Path, PathBuf};
use std::process::Command;
use crate::infra::runners::{Runner, LaunchContext, CommandSpec};
use crate::steam_client::SteamClient;
use crate::launch::pipeline::{LaunchError, LaunchErrorKind};

pub struct WineTkgRunner;

/// Outcome of the background-Steam liveness re-check performed after a readiness
/// heuristic fires and the short grace window has elapsed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReadinessGrace {
    /// Process still alive after the grace window — genuinely ready.
    Ready,
    /// Process exited within the grace window — a crash masquerading as ready.
    ExitedEarly { code: Option<i32> },
}

/// Re-check a background-Steam process after the readiness grace window.
///
/// A readiness heuristic (config.vdf/steam.pid/log files) can fire on artifacts
/// Steam writes *during* early init and then the process crashes a moment later.
/// The caller sleeps a short grace window and then invokes this: if the process
/// has since exited it is reclassified as an early exit (a crash) rather than
/// genuine readiness, so the milestone downstream matches `steam_process_exited_early`.
pub(crate) fn reclassify_after_grace(child: &mut std::process::Child) -> ReadinessGrace {
    match child.try_wait() {
        Ok(Some(status)) => ReadinessGrace::ExitedEarly { code: status.code() },
        // Still running, or the wait itself errored (treat as still-alive: the
        // subsequent launch path will surface any real problem).
        _ => ReadinessGrace::Ready,
    }
}

#[async_trait::async_trait]
impl Runner for WineTkgRunner {
    fn name(&self) -> &str { "Wine-TKG" }
    async fn prepare_prefix(&self, ctx: &LaunchContext) -> std::result::Result<(), LaunchError> {
        let library_root = PathBuf::from(&ctx.launcher_config.steam_library_path);

        let (steam_mode, runtime_source) = resolve_steam_mode(ctx);
        let use_steam_runtime = steam_mode == SteamMode::InWineRuntime;
        let steam_prefix_mode = ctx.user_config.as_ref()
            .map(|c| c.steam_prefix_mode.clone())
            .unwrap_or(ctx.launcher_config.steam_prefix_mode.clone());

        let effective_game_prefix = effective_game_prefix(ctx);
        std::fs::create_dir_all(&effective_game_prefix)
            .map_err(|e| LaunchError::new(LaunchErrorKind::Permission, format!("failed creating {}", effective_game_prefix.display())).with_source(anyhow!(e)))?;

        // Proton's `setup_prefix` opens its lock at `$STEAM_COMPAT_DATA_PATH/pfx.lock`
        // with O_CREAT, which fails with ENOENT unless the compatdata directory
        // already exists (Steam itself pre-creates it). In the default shared-prefix
        // mode the WINEPREFIX above lives elsewhere, so compatdata is never created
        // and Proton aborts immediately. Create it here so the path build_env hands
        // Proton as STEAM_COMPAT_DATA_PATH is guaranteed to exist.
        let compat_data_path = library_root
            .join("steamapps")
            .join("compatdata")
            .join(ctx.app.app_id.to_string());
        std::fs::create_dir_all(&compat_data_path)
            .map_err(|e| LaunchError::new(LaunchErrorKind::Permission, format!("failed creating {}", compat_data_path.display())).with_source(anyhow!(e)))?;

        tracing::info!("Effective game prefix: {}", effective_game_prefix.display());
        tracing::info!("Shared steam compatibility data enabled: {}", ctx.launcher_config.use_shared_compat_data);
        tracing::info!("Steam Runtime Prefix Mode: {:?}", steam_prefix_mode);

        if use_steam_runtime {
            let steam_cfg = crate::core::utils::get_master_steam_config();
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
                        )
                    ).with_context("master_prefix", steam_cfg.wine_prefix.to_string_lossy()));
                }
            };

            tracing::info!("  - Steam Exe: {}", steam_cfg.steam_exe.as_ref().unwrap().display());

            let (prefix_steam_dir, steam_wineprefix) = match steam_prefix_mode {
                        crate::core::models::SteamPrefixMode::Shared => {
                            (master_steam_dir.clone(), steam_cfg.wine_prefix.clone())
                        }
                        crate::core::models::SteamPrefixMode::PerGame => {
                            let target_steam_dir = effective_game_prefix
                                .join("drive_c/Program Files (x86)/Steam");

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
                                            let _ = crate::core::utils::copy_dir_all(&src, &dst);
                                        }
                                    }
                                    #[cfg(not(unix))]
                                    {
                                        let _ = crate::core::utils::copy_dir_all(&src, &dst);
                                    }
                                }
                            }

                    (target_steam_dir, effective_game_prefix.clone())
                }
            };

            tracing::debug!("Runtime Steam dir : {}", prefix_steam_dir.display());
                    tracing::debug!("Runtime WINEPREFIX : {}", steam_wineprefix.display());

                    SteamClient::write_headless_steam_cfg(&prefix_steam_dir);

                    let slc = ctx.user_config.as_ref()
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

                    let steam_running = SteamClient::is_steam_running_in_prefix(&steam_wineprefix);

                    unsafe {
                        if !ctx.verification_ptr.is_null() {
                            let v = &mut *ctx.verification_ptr;
                            v.steam_running_before_launch = steam_running;
                            v.effective_game_wineprefix = Some(effective_game_prefix.to_string_lossy().to_string());
                            v.effective_steam_wineprefix = Some(steam_wineprefix.to_string_lossy().to_string());
                            v.per_game_prefix_requested = steam_prefix_mode == crate::core::models::SteamPrefixMode::PerGame;
                            v.per_game_prefix_honored = effective_game_prefix == steam_wineprefix;
                            v.steam_runtime_policy = format!("{:?}", ctx.user_config.as_ref().map(|c| &c.steam_runtime_policy).unwrap_or(&crate::core::models::SteamRuntimePolicy::Auto));
                            v.steam_runtime_source = runtime_source.to_string();
                            v.windows_steam_discovery_enabled = ctx.launcher_config.windows_steam_discovery_enabled;
                        }
                    }

                    if steam_running {
                        println!("✅ Steam already running in prefix — skipping spawn");
                    } else {
                        // Background Steam is hosted on a DEDICATED Steam-runtime runner —
                        // NOT the game runner's `proton run` wrapper. It must run under a
                        // bare wine so `steam.exe` starts as a plain Windows process.
                        let mut steam_cmd = resolve_background_steam_command(ctx, &library_root)?;
                        steam_cmd.current_dir(&prefix_steam_dir);
                        steam_cmd
                            .arg("C:\\Program Files (x86)\\Steam\\steam.exe")
                            .args(&steam_args);
                        steam_cmd
                            .env("WINEPREFIX", &steam_wineprefix)
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
                        unsafe {
                            if !ctx.verification_ptr.is_null() {
                                let v = &mut *ctx.verification_ptr;
                                v.steam_runtime_exe = Some(steam_cmd.get_program().to_string_lossy().to_string());
                                v.steam_runtime_args = steam_cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
                                v.steam_runtime_milestone = "steam_process_spawn_requested".to_string();
                                v.steam_auto_start_attempted = true;
                            }
                        }

                        let start_time = std::time::Instant::now();
                        let mut steam_process =
                            steam_cmd.spawn().map_err(|e| LaunchError::new(LaunchErrorKind::Process, "Failed to spawn background Steam").with_source(anyhow!(e)))?;

                        unsafe {
                            if !ctx.verification_ptr.is_null() {
                                (*ctx.verification_ptr).steam_runtime_milestone = "steam_process_spawned".to_string();
                            }
                        }

                        let readiness_timeout = 8;
                        println!("Waiting for Steam to initialise (max {}s)...", readiness_timeout);

                        let steam_pid_path = prefix_steam_dir.join("steam.pid");
                        let steam_pipe     = steam_wineprefix.join("drive_c/windows/temp/.steampath");
                        let steam_config_vdf = prefix_steam_dir.join("config/config.vdf");
                        let steam_logs_dir   = prefix_steam_dir.join("logs");

                        let ready = 'wait: {
                            // Which readiness heuristic fired, if any. We record it and
                            // break the poll loop instead of returning `true` immediately
                            // so a short liveness grace window below can still reclassify
                            // a signal-then-crash as an early exit rather than "ready".
                            let mut ready_signal: Option<String> = None;
                            for i in 0..readiness_timeout {
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                                // Crash detection — bail immediately
                                if let Ok(Some(status)) = steam_process.try_wait() {
                                    println!("❌ FATAL: Background Steam exited after {}s with: {}", i + 1, status);
                                    unsafe {
                                        if !ctx.verification_ptr.is_null() {
                                            let v = &mut *ctx.verification_ptr;
                                            v.steam_runtime_exit_code = status.code();
                                            v.steam_runtime_lifetime_ms = Some(start_time.elapsed().as_millis() as u64);
                                            v.steam_runtime_milestone = "steam_process_exited_early".to_string();
                                        }
                                    }
                                    break 'wait false;
                                }

                                // Signal 1: pid file (some Wine/Steam combos do write this)
                                if steam_pid_path.exists() {
                                    println!("✅ Steam ready after {}s (steam.pid found)", i + 1);
                                    ready_signal = Some(format!("steam.pid after {}s", i + 1));
                                    break;
                                }

                                // Signal 2: .steampath in temp (Proton-style)
                                if steam_pipe.exists() {
                                    println!("✅ Steam ready after {}s (.steampath found)", i + 1);
                                    ready_signal = Some(format!(".steampath after {}s", i + 1));
                                    break;
                                }

                                // Signal 3: config.vdf written — Steam has finished early init
                                if steam_config_vdf.exists() {
                                    println!("✅ Steam ready after {}s (config.vdf found)", i + 1);
                                    ready_signal = Some(format!("config.vdf after {}s", i + 1));
                                    break;
                                }

                                // Signal 4: logs dir has multiple entries — Steam's subsystems are running
                                let log_count = std::fs::read_dir(&steam_logs_dir)
                                    .map(|d| d.count())
                                    .unwrap_or(0);
                                if log_count >= 2 {
                                    println!("✅ Steam ready after {}s ({} log files found)", i + 1, log_count);
                                    unsafe {
                                        if !ctx.verification_ptr.is_null() {
                                            (*ctx.verification_ptr).steam_runtime_milestone = "steam_ready_signal_observed".to_string();
                                        }
                                    }
                                    ready_signal = Some(format!("{} log files after {}s", log_count, i + 1));
                                    break;
                                }

                                println!("  Waiting... {}s", i + 1);
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
                                match reclassify_after_grace(&mut steam_process) {
                                    ReadinessGrace::ExitedEarly { code } => {
                                        println!(
                                            "❌ FATAL: Background Steam signalled ready ({}) but exited within {}s grace (code {:?})",
                                            reason, READINESS_GRACE_SECS, code
                                        );
                                        unsafe {
                                            if !ctx.verification_ptr.is_null() {
                                                let v = &mut *ctx.verification_ptr;
                                                v.steam_runtime_exit_code = code;
                                                v.steam_runtime_lifetime_ms = Some(start_time.elapsed().as_millis() as u64);
                                                v.steam_runtime_milestone = "steam_process_exited_early".to_string();
                                            }
                                        }
                                        break 'wait false;
                                    }
                                    // Still alive after the grace window — genuinely ready.
                                    ReadinessGrace::Ready => break 'wait true,
                                }
                            }

                            println!("⚠️ Steam did not signal ready after {}s, launching game anyway", readiness_timeout);
                            unsafe {
                                if !ctx.verification_ptr.is_null() {
                                    (*ctx.verification_ptr).steam_runtime_milestone = "steam_ready_timeout".to_string();
                                }
                            }
                            true
                        };

                        if !ready {
                            unsafe {
                                if !ctx.verification_ptr.is_null() {
                                    (*ctx.verification_ptr).steam_auto_start_failed = true;
                                }
                            }
                            return Err(LaunchError::new(LaunchErrorKind::Process, "Background Steam crashed before the game could start"));
                        }
                    }
        }

        // Write steam_appid.txt to the game working directory
        let (_install_dir, _executable, game_working_dir) = resolve_game_paths(ctx)?;

        let app_id_str = ctx.app.app_id.to_string();
        let app_id_path = game_working_dir.join("steam_appid.txt");
        let _ = std::fs::write(&app_id_path, &app_id_str);

        Ok(())
    }

    async fn build_env(&self, ctx: &LaunchContext) -> std::result::Result<HashMap<String, String>, LaunchError> {
        let mut env = HashMap::new();
        let app_id_str = ctx.app.app_id.to_string();

        let library_root = PathBuf::from(&ctx.launcher_config.steam_library_path);
        let compat_data_path = library_root
            .join("steamapps")
            .join("compatdata")
            .join(&app_id_str);

        let effective_game_prefix = effective_game_prefix(ctx);

        env.insert("SteamAppId".to_string(), app_id_str.clone());
        env.insert("SteamGameId".to_string(), app_id_str.clone());
        env.insert("STEAM_COMPAT_APP_ID".to_string(), app_id_str);
        env.insert("WINEPREFIX".to_string(), effective_game_prefix.to_string_lossy().to_string());
        env.insert("STEAM_COMPAT_DATA_PATH".to_string(), compat_data_path.to_string_lossy().to_string());

        // Add user identity context if available
        if let Ok(session) = crate::core::config::load_session().await {
            if let Some(steam_id) = session.steam_id {
                env.insert("SteamUser".to_string(), steam_id.to_string());
            }
            if let Some(account_name) = session.account_name {
                env.insert("SteamAppUser".to_string(), account_name);
            }
        }

        let glc = ctx.user_config.as_ref()
            .map(|c| c.graphics_layers.clone())
            .unwrap_or_default();
        let no_overlay = ctx.user_config.as_ref()
            .map(|c| c.steam_launch_config.no_overlay)
            .unwrap_or(true);

        let (_install_dir, _executable, game_working_dir) = resolve_game_paths(ctx)?;

        // Resolve proton version for component detection and DLL path building
        let proton = forced_proton(ctx)
            .or_else(|| ctx.proton_path.as_deref().filter(|p| !p.is_empty()))
            .unwrap_or("wine");

        let active_runner_path = crate::core::utils::resolve_runner(proton, &library_root);
        let _components = crate::core::utils::detect_runner_components(
            &active_runner_path,
            Some(&effective_game_prefix),
        );

        // 1. Resolve DX8-11 policy (GraphicsBackendPolicy) - CONSERVATIVE
        let (policy_dxvk, force_builtin, strict_dxvk) = match glc.graphics_backend_policy {
            // Auto is now conservative: it does NOT automatically enable DXVK
            // even if detected on disk. It prefers default Wine behavior.
            crate::core::models::GraphicsBackendPolicy::Auto => (false, false, false),
            crate::core::models::GraphicsBackendPolicy::WineD3D => (false, true, false),
            crate::core::models::GraphicsBackendPolicy::DXVK => (true, false, true),
        };

        // Manual override takes precedence if enabled
        let effective_dxvk = glc.dxvk_enabled || policy_dxvk;

        // If user explicitly selected WineD3D and didn't force DXVK, we use builtins.
        let force_builtin_d3d = force_builtin && !effective_dxvk;

        // 2. Resolve DX12 policy (D3D12ProviderPolicy) - CONSERVATIVE
        let (policy_vkd3dp, policy_vkd3dw) = match glc.d3d12_policy {
            // Auto is now conservative: no forced D3D12 provider unless explicitly requested.
            crate::core::models::D3D12ProviderPolicy::Auto => (false, false),
            crate::core::models::D3D12ProviderPolicy::Vkd3dProton => (true, false),
            crate::core::models::D3D12ProviderPolicy::Vkd3dWine => (false, true),
        };
        // Manual overrides take precedence
        let effective_vkd3d_proton = glc.vkd3d_proton_enabled || policy_vkd3dp;
        let effective_vkd3d = glc.vkd3d_enabled || policy_vkd3dw;

        // NVAPI Support
        let nvapi_enabled_cfg = ctx.user_config.as_ref().map(|c| c.graphics_layers.nvapi_enabled).unwrap_or(true);
        let nvapi_active = _components.nvapi.is_some() && nvapi_enabled_cfg;
        if nvapi_active {
            tracing::info!("NVAPI component detected and enabled, will be exposed to game");
        } else if _components.nvapi.is_some() {
            tracing::info!("NVAPI component detected but disabled by per-game settings");
        }

        // Resolve the Steam-integration mode once for the whole env build. Only the
        // host-bridge mode leaves Steam's client DLLs at Proton's defaults (so
        // `lsteamclient` bridges to the host client). The in-Wine runtime and
        // standalone modes both want the native/neutralised set (`steamclient=n`, …)
        // so the game loads the in-Wine `steamclient.dll` (or none at all).
        let (steam_mode, _steam_mode_source) = resolve_steam_mode(ctx);
        let host_bridge = steam_mode == SteamMode::HostBridge;

        let use_symlinks = glc.use_symlinks_in_prefix;
        let mut dll_overrides = crate::core::utils::build_dll_overrides(
            effective_dxvk,
            effective_vkd3d_proton,
            effective_vkd3d,
            no_overlay,
            force_builtin_d3d,
            Some(&game_working_dir),
            strict_dxvk,
            host_bridge,
        );

        // Enhance overrides with resolved DLL providers
        for res in &ctx.dll_resolutions {
            if res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::GameLocal ||
               (res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::Custom && !use_symlinks) ||
               (res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::Runner && res.name.contains("nvapi")) {

                // Do not emit overrides for DLLs that are handled via internal capabilities
                if res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::Internal {
                     tracing::info!("Resolved DLL {} is handled internally (alias), skipping explicit override", res.name);
                     continue;
                }

                // Ensure native wins for game-local or non-symlinked custom DLLs
                if !dll_overrides.contains(&format!("{}=n", res.name)) {
                     tracing::info!("Adding native override for resolved DLL: {} (provider: {:?})", res.name, res.chosen_provider);
                     dll_overrides.push_str(&format!(";{}=n", res.name));
                }
            } else if res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::Internal {
                 tracing::info!("Resolved DLL {} is handled internally (alias), skipping explicit override", res.name);
            }
        }

        // Merge auto-fixup DLL overrides from the per-game registry. Existing
        // entries (policy/provider-derived, i.e. the explicit resolution) WIN: a
        // fixup is only appended for a DLL not already present in the override string.
        if !ctx.game_fixups.dll_overrides.is_empty() {
            let existing_names: std::collections::HashSet<String> = dll_overrides
                .split(';')
                .filter_map(|e| e.split('=').next())
                .map(|s| s.trim().trim_end_matches(".dll").to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
            for (dll, mode) in &ctx.game_fixups.dll_overrides {
                let key = dll.trim_end_matches(".dll").to_lowercase();
                if existing_names.contains(&key) {
                    tracing::info!("Skipping fixup DLL override {}={} (explicit override already present)", dll, mode);
                    continue;
                }
                tracing::info!("Applying fixup DLL override: {}={}", dll, mode);
                if !dll_overrides.is_empty() {
                    dll_overrides.push(';');
                }
                dll_overrides.push_str(&format!("{}={}", dll, mode));
            }
        }

        tracing::info!("Final WINEDLLOVERRIDES: {}", dll_overrides);
        env.insert("WINEDLLOVERRIDES".to_string(), dll_overrides);

        // Track effective state for diagnostics (HACK: should ideally be done in a separate stage)
        // This is safe because WineTkgRunner is currently the only one implementing this logic.
        // We'll see if we can move it to PipelineContext later.

        // Translate Runner-resolved DLL paths into WINEDLLPATH so Wine can
        // actually find the bundled DLLs (VKD3D-Proton, DXVK, etc.) in the runner.
        // WITHOUT THIS, d3d12=n,b finds whatever is in the prefix's system32 instead.
        // CONSERVATIVE: only include paths for DLLs that are actually requested to be native.
        let mut wine_dll_dirs: Vec<String> = Vec::new();

        for res in &ctx.dll_resolutions {
            if (res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::Runner ||
                res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::Custom) && !use_symlinks
            {
                // Check if this DLL is actually selected for use by the current policy/overrides
                let name = res.name.to_lowercase();
                let is_dxvk_dll = matches!(name.as_str(), "d3d8" | "d3d9" | "d3d10" | "d3d10_1" | "d3d10core" | "d3d11" | "dxgi");
                let is_d3d12_dll = matches!(name.as_str(), "d3d12" | "d3d12core" | "libvkd3d-1" | "libvkd3d-shader-1");

                let is_nvapi_dll = matches!(name.as_str(), "nvapi" | "nvapi64" | "nvofapi64");
                let selected = (is_dxvk_dll && effective_dxvk) || (is_d3d12_dll && (effective_vkd3d_proton || effective_vkd3d)) || is_nvapi_dll;

                if !selected {
                    continue;
                }

                if let Some(path) = &res.chosen_path {
                    if let Some(parent) = path.parent() {
                        let dir = parent.to_string_lossy().to_string();
                        if !wine_dll_dirs.contains(&dir) {
                            wine_dll_dirs.push(dir);
                        }

                        // For Wine-TKG and similar layouts, we must ensure both 64-bit and 32-bit
                        // architecture folders are in WINEDLLPATH if they exist, so that both
                        // architectures of a game find their respective native DLLs.
                        let folder_name = parent.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if folder_name == "x86_64-windows" {
                            let sibling = parent.parent().unwrap().join("i386-windows");
                            if sibling.exists() {
                                let s = sibling.to_string_lossy().to_string();
                                if !wine_dll_dirs.contains(&s) {
                                    wine_dll_dirs.push(s);
                                }
                            }
                        } else if folder_name == "i386-windows" {
                            let sibling = parent.parent().unwrap().join("x86_64-windows");
                            if sibling.exists() {
                                let s = sibling.to_string_lossy().to_string();
                                if !wine_dll_dirs.contains(&s) {
                                    wine_dll_dirs.push(s);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Also add the runner's main lib/wine directories so Wine can find the
        // .dll.so PE loader stubs it needs to bridge into native DLLs. Composed from
        // the shared unified-layout constants: the `*/wine` roots plus the bare base
        // dirs (`files/lib`, `files/lib64`) where modern WOW64 builds keep their
        // unix-bridge `.so` libs (so ntdll.dll resolves in syswow64), and both the
        // `-windows` (PE) and `-unix` (WOW64 bridge) arch subdirs.
        let active_runner = crate::core::utils::resolve_runner(proton, &library_root);
        let runner_root = crate::core::utils::derive_runner_root(&active_runner);
        for lib_sub in crate::compat::proton::UNIFIED_LIB_SUBDIRS {
            let p = runner_root.join(lib_sub);
            if p.exists() {
                let s = p.to_string_lossy().to_string();
                if !wine_dll_dirs.contains(&s) {
                    wine_dll_dirs.push(s);
                }

                // Ensure architecture-specific subdirectories are also in WINEDLLPATH.
                // This is critical for PE-based runners where Wine expects DLLs in
                // x86_64-windows/i386-windows folders, and for WOW64 builds whose unix
                // bridge libs live under x86_64-unix/i386-unix.
                for arch in crate::compat::proton::ARCH_SUBDIRS {
                    let arch_p = p.join(arch);
                    if arch_p.exists() {
                        let arch_s = arch_p.to_string_lossy().to_string();
                        if !wine_dll_dirs.contains(&arch_s) {
                            wine_dll_dirs.push(arch_s);
                        }
                    }
                }
            }
        }

        if !wine_dll_dirs.is_empty() {
            // Preserve any WINEDLLPATH the user may have set in env_variables
            let existing = env.get("WINEDLLPATH").cloned().unwrap_or_default();
            let combined = if existing.is_empty() {
                wine_dll_dirs.join(":")
            } else {
                format!("{}:{}", wine_dll_dirs.join(":"), existing)
            };
            env.insert("WINEDLLPATH".to_string(), combined);
        }

        // Append runner DLL directories to WINEPATH to aid native PE loading
        let mut wine_path = vec!["C:\\Program Files (x86)\\Steam".to_string()];
        wine_path.extend(wine_dll_dirs.iter().cloned());
        env.insert("WINEPATH".to_string(), wine_path.join(";"));

        // Expose the Steam client install path the game's DRM/Steamworks bootstrap
        // reads (`STEAM_COMPAT_CLIENT_INSTALL_PATH`), matching the resolved mode:
        //   - HostBridge    → the host Steam install (Proton's lsteamclient bridges).
        //   - InWineRuntime → the in-Wine Steam runtime (real steamclient.dll +
        //                     the steam.exe prepare_prefix starts) — provides DRM
        //                     with no host Steam (issue #3).
        //   - Standalone    → the fake-Steam trap (no DRM).
        let fake_trap = |env: &mut HashMap<String, String>| -> std::result::Result<(), LaunchError> {
            let config_dir = crate::core::config::config_dir()
                .map_err(|e| LaunchError::new(LaunchErrorKind::Environment, "failed to get config dir").with_source(e))?;
            let fake_env = crate::core::utils::setup_fake_steam_trap(&config_dir)
                .map_err(|e| LaunchError::new(LaunchErrorKind::Permission, "failed to setup fake steam trap").with_source(e))?;
            env.insert("STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string(), fake_env.to_string_lossy().to_string());
            unsafe {
                if !ctx.verification_ptr.is_null() {
                    let v = &mut *ctx.verification_ptr;
                    v.steam_client_install_path_exposed_to_game = Some(fake_env.to_string_lossy().to_string());
                    v.steam_client_install_path_source = Some("fake_trap".to_string());
                }
            }
            Ok(())
        };
        let expose = |env: &mut HashMap<String, String>, path: PathBuf, source: &str| {
            env.insert("STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string(), path.to_string_lossy().to_string());
            unsafe {
                if !ctx.verification_ptr.is_null() {
                    let v = &mut *ctx.verification_ptr;
                    v.steam_client_install_path_exposed_to_game = Some(path.to_string_lossy().to_string());
                    v.steam_client_install_path_source = Some(source.to_string());
                }
            }
        };

        match steam_mode {
            SteamMode::HostBridge => match crate::core::utils::host_steam_client_path() {
                Some(path) => expose(&mut env, path, "real_host"),
                // resolve_steam_mode only picks HostBridge when host Steam exists, so
                // this is defensive (a Steam install vanishing mid-launch).
                None => fake_trap(&mut env)?,
            },
            SteamMode::InWineRuntime => {
                let steam_cfg = crate::core::utils::get_master_steam_config();
                let steam_prefix_mode = ctx.user_config.as_ref()
                    .map(|c| c.steam_prefix_mode.clone())
                    .unwrap_or(ctx.launcher_config.steam_prefix_mode.clone());

                let steam_client_path = match steam_prefix_mode {
                    crate::core::models::SteamPrefixMode::Shared => {
                        steam_cfg.steam_exe.as_ref().and_then(|e| e.parent().map(|p| p.to_path_buf()))
                    }
                    crate::core::models::SteamPrefixMode::PerGame => {
                        Some(effective_game_prefix.join("drive_c/Program Files (x86)/Steam"))
                    }
                };

                match steam_client_path {
                    Some(path) => expose(&mut env, path, "real"),
                    None => fake_trap(&mut env)?,
                }
            }
            SteamMode::Standalone => {
                if ctx.steam_enabled {
                    tracing::warn!(
                        "--steam was requested but neither a host Steam client nor the in-Wine \
                         Steam runtime is installed; launching standalone. DRM-protected games \
                         will not run. Install the runtime with `aurelia steam-runtime install`."
                    );
                }
                fake_trap(&mut env)?;
            }
        }

        if let Ok(display) = std::env::var("DISPLAY") {
            env.insert("DISPLAY".to_string(), display);
        }
        if let Ok(wayland) = std::env::var("WAYLAND_DISPLAY") {
            env.insert("WAYLAND_DISPLAY".to_string(), wayland);
        }
        if let Ok(xdg_runtime) = std::env::var("XDG_RUNTIME_DIR") {
            env.insert("XDG_RUNTIME_DIR".to_string(), xdg_runtime);
        }
        // X11 servers that use cookie authentication reject Wine's winex11 driver
        // unless XAUTHORITY is forwarded. Modern desktops (GDM/systemd/Xwayland)
        // place the cookie under $XDG_RUNTIME_DIR rather than ~/.Xauthority, so
        // Wine can't find it implicitly — without this the game fails to create a
        // window ("nodrv_CreateWindow: no driver could be loaded") and runs
        // invisibly even though DISPLAY is set.
        if let Ok(xauthority) = std::env::var("XAUTHORITY") {
            env.insert("XAUTHORITY".to_string(), xauthority);
        }

        // Apply GPU preference if specified. CONSERVATIVE: No forced offload if unset.
        if let Some(gpu_pref) = ctx.user_config.as_ref().and_then(|c| c.gpu_preference.as_ref()) {
            let available_gpus = crate::core::utils::list_available_gpus();
            if let Some(gpu) = available_gpus.iter().find(|g| &g.name == gpu_pref) {
                if gpu.name.contains("NVIDIA") {
                    env.insert("__NV_PRIME_RENDER_OFFLOAD".to_string(), "1".to_string());
                    env.insert("__NV_PRIME_RENDER_OFFLOAD_PROVIDER".to_string(), "NVIDIA-G0".to_string());
                    env.insert("__VK_LAYER_NV_optimus".to_string(), "NVIDIA_only".to_string());
                    env.insert("__GLX_VENDOR_LIBRARY_NAME".to_string(), "nvidia".to_string());
                } else if gpu.name.contains("AMD") || gpu.name.contains("Intel") || gpu.name.contains("Unknown") {
                    // Standard DRI_PRIME for non-NVIDIA discrete/specific GPUs
                    // Try to find "cardN" and extract N
                    static CARD_RE: std::sync::LazyLock<regex::Regex> =
                        std::sync::LazyLock::new(|| regex::Regex::new(r"card(\d+)").unwrap());
                    if let Some(card_idx) = CARD_RE.captures(&gpu.name)
                        .and_then(|caps| caps.get(1))
                        .and_then(|m| m.as_str().parse::<u32>().ok())
                    {
                        // DRI_PRIME=1 is the most common way to select the second GPU
                        // For now we use the standard PRIME offload if it's not card0.
                        let dri_prime = if card_idx > 0 { "1" } else { "0" };
                        env.insert("DRI_PRIME".to_string(), dri_prime.to_string());
                    }
                }
            }
        }

        // Merge auto-fixup env vars from the per-game registry. Applied BEFORE the
        // user's explicit `env_variables` below so that on conflict the user's value
        // wins (an explicit per-game env var overwrites the auto-fixup).
        if !ctx.game_fixups.env.is_empty() {
            let user_has = |key: &str| {
                ctx.user_config
                    .as_ref()
                    .is_some_and(|c| c.env_variables.contains_key(key))
            };
            for (key, val) in &ctx.game_fixups.env {
                if user_has(key) {
                    tracing::info!("Skipping fixup env {} (explicit per-game value present)", key);
                    continue;
                }
                tracing::info!("Applying fixup env: {}={}", key, val);
                env.insert(key.clone(), val.clone());
            }
        }

        if let Some(config) = &ctx.user_config {
            for (key, val) in &config.env_variables {
                env.insert(key.clone(), val.clone());
            }

            // Add debug toggles
            if effective_dxvk && !env.contains_key("DXVK_HUD") {
                env.insert("DXVK_HUD".to_string(), "compiler".to_string());
            }
            if (effective_vkd3d_proton || effective_vkd3d) && !env.contains_key("VKD3D_DEBUG") {
                env.insert("VKD3D_DEBUG".to_string(), "warn".to_string());
            }
        }

        let wants_mangohud = ctx.user_config.as_ref()
            .map(|c| {
                c.env_variables.contains_key("MANGOHUD")
                    || c.launch_options
                        .split_whitespace()
                        .any(|a| a == "-mangohud" || a == "--mangohud")
            })
            .unwrap_or(false);

        if wants_mangohud {
            match SteamClient::find_mangohud_lib() {
                Some(lib) => {
                    let existing = std::env::var("LD_PRELOAD").unwrap_or_default();
                    let new_preload = if existing.is_empty() {
                        lib.to_string_lossy().to_string()
                    } else {
                        format!("{}:{}", lib.to_string_lossy(), existing)
                    };
                    env.insert("LD_PRELOAD".to_string(), new_preload);
                    env.insert("MANGOHUD".to_string(), "1".to_string());
                    env.insert("MANGOHUD_DLSYM".to_string(), "1".to_string());
                }
                None => {
                    println!("⚠️  MangoHud requested but libMangoHud.so not found — skipping");
                }
            }
        }

        env.insert("WINEDEBUG".to_string(), "err+all,warn+module,warn+loaddll".to_string());

        let log_dir = crate::core::config::config_dir()
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
            .join("logs");
        let log_path = log_dir.join(format!("wine_{}.log", ctx.app.app_id));
        env.insert("WINE_LOG_OUTPUT".to_string(), log_path.to_string_lossy().to_string());

        Ok(env)
    }

    async fn build_command(&self, ctx: &LaunchContext) -> std::result::Result<CommandSpec, LaunchError> {
        let library_root = PathBuf::from(&ctx.launcher_config.steam_library_path);

        let proton = resolve_proton_required(ctx)?;
        let active_runner = crate::core::utils::resolve_runner(proton, &library_root);

        // Classify the GAME runner to record the launch route. A real Proton tree is
        // launched via its `proton run` script (protonfixes apply naturally); plain
        // Wine / wine-tkg launch via bare wine and rely on the data-driven fixup layer
        // (merged in build_env). In every case the game binary is a DIRECT spawn arg
        // to the runner — never a `steam://run/<appid>` or `-applaunch` handoff.
        match crate::core::utils::classify_runner(&active_runner) {
            crate::core::utils::RunnerKind::Proton => {
                tracing::info!("Game runner classified as Proton: launching via `proton run` (protonfixes active)");
            }
            crate::core::utils::RunnerKind::WineTkg | crate::core::utils::RunnerKind::PlainWine => {
                tracing::info!(
                    fixup_env = ctx.game_fixups.env.len(),
                    fixup_dll = ctx.game_fixups.dll_overrides.len(),
                    "Game runner classified as bare Wine: launching via wine + fixup layer"
                );
            }
            crate::core::utils::RunnerKind::Unknown => {
                tracing::warn!(
                    "Game runner '{}' did not classify as Proton or Wine; relying on build_runner_command resolution",
                    active_runner.display()
                );
            }
        }

        // umu-launcher (when active) wraps the launch instead of resolving a runner
        // command below; it selects Proton via PROTONPATH and spawns `umu-run` directly.
        // The plugin-resolved absolute `umu-run` path is threaded in via the context.
        let use_umu = ctx.use_umu;

        let mut spec = CommandSpec::default();

        if use_umu {
            // umu-launcher is the compatibility wrapper: it invokes Proton itself
            // (selected via PROTONPATH) so we spawn `umu-run` directly with the game
            // executable — NO 'proton run' prefix and NO steam:// handoff. The umu-run
            // binary is resolved by the umu plugin (ResolveComponentsStage).
            let umu_run = ctx.umu_run.clone().ok_or_else(|| {
                LaunchError::new(
                    LaunchErrorKind::Runner,
                    "umu is active but the plugin `umu-run` path was not resolved",
                )
            })?;
            spec.program = umu_run;
        } else {
            // Build the base command (handles 'proton run' wrapper and directory resolution)
            let base_cmd = crate::core::utils::build_runner_command(&active_runner)
                .map_err(|e| LaunchError::new(LaunchErrorKind::Runner, format!("Invalid Compatibility Layer path: {}", active_runner.display())).with_source(e))?;
            spec.program = base_cmd.get_program().into();
            spec.args = base_cmd.get_args().map(|s| s.to_string_lossy().to_string()).collect();
        }

        let (_install_dir, executable, game_working_dir) = resolve_game_paths(ctx)?;

        spec.cwd = Some(game_working_dir);
        spec.args.push(executable.to_string_lossy().to_string());

        // Split args from launch_info
        let args = ctx.launch_info.arguments.split_whitespace().map(ToString::to_string);
        spec.args.extend(args);

        // Split user launch args
        let user_launch_args = ctx.user_config.as_ref()
            .map(|c| c.launch_options.split_whitespace().map(ToString::to_string).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a != "-mangohud" && a != "--mangohud");
        spec.args.extend(user_launch_args);

        spec.env = self.build_env(ctx).await?;

        if use_umu {
            // umu-run needs GAMEID (the Steam AppID) and PROTONPATH (the absolute
            // directory of the resolved Proton tool) to select and run the game.
            let proton_root = crate::core::utils::derive_runner_root(&active_runner);
            spec.env.insert("GAMEID".to_string(), ctx.app.app_id.to_string());
            spec.env.insert("PROTONPATH".to_string(), proton_root.to_string_lossy().to_string());
        }

        Ok(spec)
    }

    fn launch(&self, spec: &CommandSpec) -> std::result::Result<std::process::Child, LaunchError> {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        for (key, val) in &spec.env {
            cmd.env(key, val);
        }

        let log_path = spec.env.get("WINE_LOG_OUTPUT").map(PathBuf::from);
        if let Some(path) = log_path {
            std::fs::create_dir_all(path.parent().unwrap()).ok();
            if let Ok(log_file) = std::fs::File::create(&path) {
                cmd.stderr(log_file);
            } else {
                cmd.stderr(std::process::Stdio::inherit());
            }
        } else {
            cmd.stderr(std::process::Stdio::inherit());
        }

        cmd.stdout(std::process::Stdio::inherit());

        tracing::debug!(
            program = ?cmd.get_program(),
            args = ?cmd.get_args().collect::<Vec<_>>(),
            working_dir = ?cmd.get_current_dir(),
            "runner launch",
        );

        cmd.spawn().map_err(|e| LaunchError::new(LaunchErrorKind::Process, "failed to spawn runner process").with_source(anyhow!(e)))
    }
}

/// Build the effective Wine prefix for the game from the launch context.
///
/// Wraps this game's per-app config (if any) in a single-entry `UserConfigStore`
/// and resolves the prefix the same way the launcher does globally.
fn effective_game_prefix(ctx: &LaunchContext) -> PathBuf {
    let user_config_store: crate::core::models::UserConfigStore = ctx.user_config.as_ref().map(|c| {
        let mut store = HashMap::new();
        store.insert(ctx.app.app_id, c.clone());
        store
    }).unwrap_or_default().into();

    crate::core::utils::steam_wineprefix_for_game(
        &ctx.launcher_config,
        ctx.app.app_id,
        &user_config_store,
    )
}

/// Which Steam-integration mode a launch runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SteamMode {
    /// Bridge to the **host** Steam client via Proton's `lsteamclient` (full online
    /// Steamworks, Family-Shared licences). Chosen for `--steam` when a host Steam
    /// install is present. DLL overrides stay at Proton's defaults so `lsteamclient`
    /// loads builtin and bridges to the running client.
    HostBridge,
    /// Run against the Windows Steam runtime installed **inside Wine**
    /// (`aurelia steam-runtime install`): the game loads the real `steamclient.dll`
    /// and connects to the in-Wine `steam.exe` Aurelia starts. Provides Steam DRM
    /// (and offline Steamworks) with no host Steam install.
    InWineRuntime,
    /// No Steam client — the standalone fake-Steam trap. DRM-protected games can't run.
    Standalone,
}

/// Resolve the Steam-integration mode, reconciling the `--steam` flag with the
/// configurable [`SteamRuntimePolicy`] and which Steam installs are present.
///
/// The policy is the authoritative knob, resolved per-game first
/// (`aurelia config game <id> --steam-runtime …`) and, when that is `Auto`, from the
/// global default (`aurelia config steam-runtime-policy …`):
///   - `Enabled`  — always use the in-Wine Steam runtime (real `steamclient.dll` +
///                  the `steam.exe` Aurelia starts), even when a host Steam exists.
///   - `Disabled` — never use the in-Wine runtime; `--steam` bridges to host Steam.
///   - `Auto`     — `--steam` prefers the host client and falls back to the in-Wine
///                  runtime when no host Steam is installed (previously it assumed a
///                  host Steam and silently ran DRM-protected games without DRM,
///                  issue #3). Without `--steam`, standalone.
///
/// Returns the mode and a short source label recorded in launch diagnostics.
fn resolve_steam_mode(ctx: &LaunchContext) -> (SteamMode, &'static str) {
    use crate::core::models::SteamRuntimePolicy;

    // Per-game policy wins; `Auto` inherits the global default. The deprecated
    // per-game `use_steam_runtime` boolean still forces the runtime on.
    let per_game = ctx
        .user_config
        .as_ref()
        .map(|c| c.steam_runtime_policy)
        .unwrap_or_default();
    let legacy_forced = ctx.user_config.as_ref().map(|c| c.use_steam_runtime).unwrap_or(false);
    let effective = match per_game {
        SteamRuntimePolicy::Auto if legacy_forced => SteamRuntimePolicy::Enabled,
        SteamRuntimePolicy::Auto => ctx.launcher_config.steam_runtime_policy,
        explicit => explicit,
    };

    let master_installed =
        || crate::core::utils::get_master_steam_config().steam_exe.is_some();
    let host_installed = || crate::core::utils::host_steam_client_path().is_some();

    match effective {
        // Explicitly configured to use the in-Wine runtime.
        SteamRuntimePolicy::Enabled => {
            if master_installed() {
                (SteamMode::InWineRuntime, "policy_enabled")
            } else if ctx.steam_enabled && host_installed() {
                // Configured for the runtime but it isn't installed — honour --steam
                // by bridging to host Steam rather than silently disabling DRM.
                (SteamMode::HostBridge, "policy_enabled_host_fallback")
            } else {
                (SteamMode::Standalone, "policy_enabled_no_runtime")
            }
        }
        // In-Wine runtime forbidden: --steam uses host Steam (or standalone).
        SteamRuntimePolicy::Disabled => {
            if ctx.steam_enabled && host_installed() {
                (SteamMode::HostBridge, "host")
            } else if ctx.steam_enabled {
                (SteamMode::Standalone, "steam_flag_no_host")
            } else {
                (SteamMode::Standalone, "policy_disabled")
            }
        }
        // Auto: --steam prefers host, else the in-Wine runtime, else standalone.
        SteamRuntimePolicy::Auto => {
            if !ctx.steam_enabled {
                (SteamMode::Standalone, "default")
            } else if host_installed() {
                (SteamMode::HostBridge, "host")
            } else if master_installed() {
                (SteamMode::InWineRuntime, "steam_flag_no_host")
            } else {
                (SteamMode::Standalone, "steam_flag_no_steam")
            }
        }
    }
}

/// Per-game forced Proton/runner override, if one is configured.
fn forced_proton(ctx: &LaunchContext) -> Option<&str> {
    ctx.launcher_config
        .game_configs
        .get(&ctx.app.app_id)
        .and_then(|c| c.forced_proton_version.as_ref())
        .map(|s| s.as_str())
}

/// Resolve the Proton/runner identifier, erroring if none is available.
///
/// Prefers a per-game forced version, then the context's `proton_path`.
fn resolve_proton_required(ctx: &LaunchContext) -> std::result::Result<&str, LaunchError> {
    if let Some(forced) = forced_proton(ctx) {
        Ok(forced)
    } else {
        ctx.proton_path.as_deref()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| LaunchError::new(LaunchErrorKind::Environment, "proton path is required for Windows launch"))
    }
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
        let bare_wine = crate::core::utils::resolve_steam_runtime_wine(&name, library_root)
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
    let game_runner = crate::core::utils::resolve_runner(proton, library_root);
    match crate::core::utils::classify_runner(&game_runner) {
        crate::core::utils::RunnerKind::WineTkg | crate::core::utils::RunnerKind::PlainWine => {
            tracing::info!("Background Steam runner: bare wine tree {}", game_runner.display());
            crate::core::utils::build_runner_command(&game_runner).map_err(|e| {
                LaunchError::new(
                    LaunchErrorKind::Runner,
                    format!("Invalid Compatibility Layer path: {}", game_runner.display()),
                )
                .with_source(e)
            })
        }
        crate::core::utils::RunnerKind::Proton => {
            // Only a Proton tree is available — use its bundled bare wine directly,
            // NOT `proton run` (the protonfixes wrapper is wrong for background Steam).
            let bare = crate::core::utils::proton_bundled_bare_wine(&game_runner).ok_or_else(|| {
                LaunchError::new(
                    LaunchErrorKind::Environment,
                    format!(
                        "the only available runtime is a Proton tree ({}) but its bundled bare wine \
                         (files/bin/wine64) could not be found; background Steam needs a bare wine",
                        game_runner.display()
                    ),
                )
            })?;
            tracing::info!("Background Steam runner: Proton-bundled bare wine {}", bare.display());
            Ok(Command::new(bare))
        }
        crate::core::utils::RunnerKind::Unknown => Err(LaunchError::new(
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

/// Resolve `(install_dir, executable, game_working_dir)` for the game.
///
/// Errors if the game is not installed. The executable is resolved relative to
/// the install dir unless it is absolute; the working dir honours an explicit
/// `workingdir`, then the executable's parent, then the install dir.
fn resolve_game_paths(ctx: &LaunchContext) -> std::result::Result<(PathBuf, PathBuf, PathBuf), LaunchError> {
    let install_dir = PathBuf::from(
        ctx.app.install_path
            .clone()
            .ok_or_else(|| LaunchError::new(LaunchErrorKind::GameData, format!("game {} is not installed", ctx.app.app_id)))?,
    );

    let exe_rel = ctx.launch_info.executable.replace('\\', "/");
    let executable = if Path::new(&exe_rel).is_absolute() {
        PathBuf::from(&exe_rel)
    } else {
        install_dir.join(&exe_rel)
    };
    let game_working_dir: PathBuf = ctx.launch_info.workingdir
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|wd| install_dir.join(wd.replace('\\', "/")))
        .or_else(|| executable.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| install_dir.clone());

    Ok((install_dir, executable, game_working_dir))
}

