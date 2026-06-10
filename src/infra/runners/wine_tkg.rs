use std::collections::HashMap;
use anyhow::anyhow;
use std::path::{Path, PathBuf};
use std::process::Command;
use crate::infra::runners::{Runner, LaunchContext, CommandSpec};
use crate::steam_client::SteamClient;
use crate::launch::pipeline::{LaunchError, LaunchErrorKind};

pub struct WineTkgRunner;

#[async_trait::async_trait]
impl Runner for WineTkgRunner {
    fn name(&self) -> &str { "Wine-TKG" }
    async fn prepare_prefix(&self, ctx: &LaunchContext) -> std::result::Result<(), LaunchError> {
        let library_root = PathBuf::from(&ctx.launcher_config.steam_library_path);

        let (use_steam_runtime, runtime_source) = match ctx.user_config.as_ref().map(|c| &c.steam_runtime_policy) {
            Some(crate::models::SteamRuntimePolicy::Enabled) => (true, "override"),
            Some(crate::models::SteamRuntimePolicy::Disabled) => (false, "override"),
            Some(crate::models::SteamRuntimePolicy::Auto) | None => {
                // Fallback to deprecated boolean if policy is Auto/None for backward compat
                let manual_toggle = ctx.user_config.as_ref().map(|c| c.use_steam_runtime).unwrap_or(false);
                if manual_toggle {
                    (true, "override_legacy")
                } else {
                    (false, "default")
                }
            }
        };
        let steam_prefix_mode = ctx.user_config.as_ref()
            .map(|c| c.steam_prefix_mode.clone())
            .unwrap_or(ctx.launcher_config.steam_prefix_mode.clone());

        let user_config_store: crate::models::UserConfigStore = ctx.user_config.as_ref().map(|c| {
            let mut store = HashMap::new();
            store.insert(ctx.app.app_id, c.clone());
            store
        }).unwrap_or_default().into();

        let effective_game_prefix = crate::utils::steam_wineprefix_for_game(
            &ctx.launcher_config,
            ctx.app.app_id,
            &user_config_store
        );
        std::fs::create_dir_all(&effective_game_prefix)
            .map_err(|e| LaunchError::new(LaunchErrorKind::Permission, format!("failed creating {}", effective_game_prefix.display())).with_source(anyhow!(e)))?;

        tracing::info!("Effective game prefix: {}", effective_game_prefix.display());
        tracing::info!("Shared steam compatibility data enabled: {}", ctx.launcher_config.use_shared_compat_data);
        tracing::info!("Steam Runtime Prefix Mode: {:?}", steam_prefix_mode);

        if use_steam_runtime {
            let steam_cfg = crate::utils::get_master_steam_config();
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
                        crate::models::SteamPrefixMode::Shared => {
                            (master_steam_dir.clone(), steam_cfg.wine_prefix.clone())
                        }
                        crate::models::SteamPrefixMode::PerGame => {
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
                                            let _ = crate::utils::copy_dir_all(&src, &dst);
                                        }
                                    }
                                    #[cfg(not(unix))]
                                    {
                                        let _ = crate::utils::copy_dir_all(&src, &dst);
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
                            v.per_game_prefix_requested = steam_prefix_mode == crate::models::SteamPrefixMode::PerGame;
                            v.per_game_prefix_honored = effective_game_prefix == steam_wineprefix;
                            v.steam_runtime_policy = format!("{:?}", ctx.user_config.as_ref().map(|c| &c.steam_runtime_policy).unwrap_or(&crate::models::SteamRuntimePolicy::Auto));
                            v.steam_runtime_source = runtime_source.to_string();
                            v.windows_steam_discovery_enabled = ctx.launcher_config.windows_steam_discovery_enabled;
                        }
                    }

                    if steam_running {
                        println!("✅ Steam already running in prefix — skipping spawn");
                    } else {
                        let proton = if let Some(forced) = ctx.launcher_config
                            .game_configs
                            .get(&ctx.app.app_id)
                            .and_then(|c| c.forced_proton_version.as_ref())
                        {
                            forced.as_str()
                        } else {
                            ctx.proton_path.as_deref()
                                .filter(|p| !p.is_empty())
                                .ok_or_else(|| LaunchError::new(LaunchErrorKind::Environment, "proton path is required for Windows launch"))?
                        };
                        let active_runner = crate::utils::resolve_runner(proton, &library_root);

                        let mut steam_cmd = crate::utils::build_runner_command(&active_runner)
                            .map_err(|e| LaunchError::new(LaunchErrorKind::Runner, format!("Invalid Compatibility Layer path: {}", active_runner.display())).with_source(e))?;
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

                        println!("Program: {:?}", steam_cmd.get_program());
                        println!("Args: {:?}", steam_cmd.get_args().collect::<Vec<_>>());
                        println!("--------------------------");

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
                                    break 'wait true;
                                }

                                // Signal 2: .steampath in temp (Proton-style)
                                if steam_pipe.exists() {
                                    println!("✅ Steam ready after {}s (.steampath found)", i + 1);
                                    break 'wait true;
                                }

                                // Signal 3: config.vdf written — Steam has finished early init
                                if steam_config_vdf.exists() {
                                    println!("✅ Steam ready after {}s (config.vdf found)", i + 1);
                                    break 'wait true;
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
                                    break 'wait true;
                                }

                                println!("  Waiting... {}s", i + 1);
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

        let user_config_store: crate::models::UserConfigStore = ctx.user_config.as_ref().map(|c| {
            let mut store = HashMap::new();
            store.insert(ctx.app.app_id, c.clone());
            store
        }).unwrap_or_default().into();

        let effective_game_prefix = crate::utils::steam_wineprefix_for_game(
            &ctx.launcher_config,
            ctx.app.app_id,
            &user_config_store
        );

        env.insert("SteamAppId".to_string(), app_id_str.clone());
        env.insert("SteamGameId".to_string(), app_id_str.clone());
        env.insert("STEAM_COMPAT_APP_ID".to_string(), app_id_str);
        env.insert("WINEPREFIX".to_string(), effective_game_prefix.to_string_lossy().to_string());
        env.insert("STEAM_COMPAT_DATA_PATH".to_string(), compat_data_path.to_string_lossy().to_string());

        // Add user identity context if available
        if let Ok(session) = crate::config::load_session().await {
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

        let game_working_dir: PathBuf = {
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
            ctx.launch_info.workingdir
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|wd| install_dir.join(wd.replace('\\', "/")))
                .or_else(|| executable.parent().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| install_dir.clone())
        };

        // Resolve proton version for component detection and DLL path building
        let proton = if let Some(forced) = ctx.launcher_config
            .game_configs
            .get(&ctx.app.app_id)
            .and_then(|c| c.forced_proton_version.as_ref())
        {
            forced.as_str()
        } else {
            ctx.proton_path.as_deref()
                .filter(|p| !p.is_empty())
                .unwrap_or("wine")
        };

        let active_runner_path = crate::utils::resolve_runner(proton, &library_root);
        let _components = crate::utils::detect_runner_components(
            &active_runner_path,
            Some(&effective_game_prefix),
        );

        // 1. Resolve DX8-11 policy (GraphicsBackendPolicy) - CONSERVATIVE
        let (policy_dxvk, force_builtin, strict_dxvk) = match glc.graphics_backend_policy {
            // Auto is now conservative: it does NOT automatically enable DXVK
            // even if detected on disk. It prefers default Wine behavior.
            crate::models::GraphicsBackendPolicy::Auto => (false, false, false),
            crate::models::GraphicsBackendPolicy::WineD3D => (false, true, false),
            crate::models::GraphicsBackendPolicy::DXVK => (true, false, true),
        };

        // Manual override takes precedence if enabled
        let effective_dxvk = glc.dxvk_enabled || policy_dxvk;

        // If user explicitly selected WineD3D and didn't force DXVK, we use builtins.
        let force_builtin_d3d = force_builtin && !effective_dxvk;

        // 2. Resolve DX12 policy (D3D12ProviderPolicy) - CONSERVATIVE
        let (policy_vkd3dp, policy_vkd3dw) = match glc.d3d12_policy {
            // Auto is now conservative: no forced D3D12 provider unless explicitly requested.
            crate::models::D3D12ProviderPolicy::Auto => (false, false),
            crate::models::D3D12ProviderPolicy::Vkd3dProton => (true, false),
            crate::models::D3D12ProviderPolicy::Vkd3dWine => (false, true),
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

        let use_symlinks = glc.use_symlinks_in_prefix;
        let mut dll_overrides = crate::utils::build_dll_overrides(
            effective_dxvk,
            effective_vkd3d_proton,
            effective_vkd3d,
            no_overlay,
            force_builtin_d3d,
            Some(&game_working_dir),
            strict_dxvk,
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
        let use_symlinks = glc.use_symlinks_in_prefix;

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

        // Also add the runner's main lib/wine directories so Wine can find
        // the .dll.so PE loader stubs it needs to bridge into native DLLs.
        let active_runner = crate::utils::resolve_runner(proton, &library_root);
        let runner_root = crate::utils::derive_runner_root(&active_runner);
        for lib_sub in &[
            "lib/wine",
            "lib64/wine",
            "files/lib/wine",
            "files/lib64/wine",
            "dist/lib/wine",
            "dist/lib64/wine",
        ] {
            let p = runner_root.join(lib_sub);
            if p.exists() {
                let s = p.to_string_lossy().to_string();
                if !wine_dll_dirs.contains(&s) {
                    wine_dll_dirs.push(s);
                }

                // Ensure architecture-specific subdirectories are also in WINEDLLPATH.
                // This is critical for PE-based runners where Wine expects DLLs in
                // x86_64-windows or i386-windows folders even for the main runner libs.
                for arch in &["x86_64-windows", "i386-windows"] {
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

        let mut wine_path = vec!["C:\\Program Files (x86)\\Steam".to_string()];
        // Append runner DLL directories to WINEPATH to aid native PE loading
        for dir in &wine_dll_dirs {
            wine_path.push(dir.clone());
        }
        env.insert("WINEPATH".to_string(), wine_path.join(";"));

        let (use_steam_runtime, _runtime_source) = match ctx.user_config.as_ref().map(|c| &c.steam_runtime_policy) {
            Some(crate::models::SteamRuntimePolicy::Enabled) => (true, "override"),
            Some(crate::models::SteamRuntimePolicy::Disabled) => (false, "override"),
            Some(crate::models::SteamRuntimePolicy::Auto) | None => {
                let manual_toggle = ctx.user_config.as_ref().map(|c| c.use_steam_runtime).unwrap_or(false);
                if manual_toggle {
                    (true, "override_legacy")
                } else {
                    (false, "default")
                }
            }
        };

        if use_steam_runtime {
            let steam_cfg = crate::utils::get_master_steam_config();
            let steam_prefix_mode = ctx.user_config.as_ref()
                .map(|c| c.steam_prefix_mode.clone())
                .unwrap_or(ctx.launcher_config.steam_prefix_mode.clone());

            let steam_client_path = match steam_prefix_mode {
                crate::models::SteamPrefixMode::Shared => {
                    steam_cfg.steam_exe.as_ref().and_then(|e| e.parent().map(|p| p.to_path_buf()))
                }
                crate::models::SteamPrefixMode::PerGame => {
                    Some(effective_game_prefix.join("drive_c/Program Files (x86)/Steam"))
                }
            };

            if let Some(path) = steam_client_path {
                env.insert("STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string(), path.to_string_lossy().to_string());
                unsafe {
                    if !ctx.verification_ptr.is_null() {
                        let v = &mut *ctx.verification_ptr;
                        v.steam_client_install_path_exposed_to_game = Some(path.to_string_lossy().to_string());
                        v.steam_client_install_path_source = Some("real".to_string());
                    }
                }
            } else {
                let config_dir = crate::config::config_dir().map_err(|e| LaunchError::new(LaunchErrorKind::Environment, "failed to get config dir").with_source(e))?;
                let fake_env = crate::utils::setup_fake_steam_trap(&config_dir)
                    .map_err(|e| LaunchError::new(LaunchErrorKind::Permission, "failed to setup fake steam trap").with_source(e))?;
                env.insert("STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string(), fake_env.to_string_lossy().to_string());
                unsafe {
                    if !ctx.verification_ptr.is_null() {
                        let v = &mut *ctx.verification_ptr;
                        v.steam_client_install_path_exposed_to_game = Some(fake_env.to_string_lossy().to_string());
                        v.steam_client_install_path_source = Some("fake_trap".to_string());
                    }
                }
            }
        } else {
            let config_dir = crate::config::config_dir().map_err(|e| LaunchError::new(LaunchErrorKind::Environment, "failed to get config dir").with_source(e))?;
            let fake_env = crate::utils::setup_fake_steam_trap(&config_dir)
                .map_err(|e| LaunchError::new(LaunchErrorKind::Permission, "failed to setup fake steam trap").with_source(e))?;
            env.insert("STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string(), fake_env.to_string_lossy().to_string());
            unsafe {
                if !ctx.verification_ptr.is_null() {
                    let v = &mut *ctx.verification_ptr;
                    v.steam_client_install_path_exposed_to_game = Some(fake_env.to_string_lossy().to_string());
                    v.steam_client_install_path_source = Some("fake_trap".to_string());
                }
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

        // Apply GPU preference if specified. CONSERVATIVE: No forced offload if unset.
        if let Some(gpu_pref) = ctx.user_config.as_ref().and_then(|c| c.gpu_preference.as_ref()) {
            let available_gpus = crate::utils::list_available_gpus();
            if let Some(gpu) = available_gpus.iter().find(|g| &g.name == gpu_pref) {
                if gpu.name.contains("NVIDIA") {
                    env.insert("__NV_PRIME_RENDER_OFFLOAD".to_string(), "1".to_string());
                    env.insert("__NV_PRIME_RENDER_OFFLOAD_PROVIDER".to_string(), "NVIDIA-G0".to_string());
                    env.insert("__VK_LAYER_NV_optimus".to_string(), "NVIDIA_only".to_string());
                    env.insert("__GLX_VENDOR_LIBRARY_NAME".to_string(), "nvidia".to_string());
                } else if gpu.name.contains("AMD") || gpu.name.contains("Intel") || gpu.name.contains("Unknown") {
                    // Standard DRI_PRIME for non-NVIDIA discrete/specific GPUs
                    // Try to find "cardN" and extract N
                    let re = regex::Regex::new(r"card(\d+)").unwrap();
                    if let Some(caps) = re.captures(&gpu.name) {
                        if let Some(idx_match) = caps.get(1) {
                            if let Ok(card_idx) = idx_match.as_str().parse::<u32>() {
                                 // DRI_PRIME=1 is the most common way to select the second GPU
                                 // For now we use the standard PRIME offload if it's not card0.
                                 if card_idx > 0 {
                                     env.insert("DRI_PRIME".to_string(), "1".to_string());
                                 } else {
                                     env.insert("DRI_PRIME".to_string(), "0".to_string());
                                 }
                            }
                        }
                    }
                }
            }
        }

        if let Some(config) = &ctx.user_config {
            for (key, val) in &config.env_variables {
                env.insert(key.clone(), val.clone());
            }

            // Add debug toggles
            if effective_dxvk {
                if !env.contains_key("DXVK_HUD") {
                    env.insert("DXVK_HUD".to_string(), "compiler".to_string());
                }
            }
            if effective_vkd3d_proton || effective_vkd3d {
                 if !env.contains_key("VKD3D_DEBUG") {
                    env.insert("VKD3D_DEBUG".to_string(), "warn".to_string());
                }
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
            let lib_path = SteamClient::find_mangohud_lib();
            match lib_path {
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

        let log_dir = crate::config::config_dir()
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
            .join("logs");
        let log_path = log_dir.join(format!("wine_{}.log", ctx.app.app_id));
        env.insert("WINE_LOG_OUTPUT".to_string(), log_path.to_string_lossy().to_string());

        Ok(env)
    }

    async fn build_command(&self, ctx: &LaunchContext) -> std::result::Result<CommandSpec, LaunchError> {
        let library_root = PathBuf::from(&ctx.launcher_config.steam_library_path);

        let proton = if let Some(forced) = ctx.launcher_config
            .game_configs
            .get(&ctx.app.app_id)
            .and_then(|c| c.forced_proton_version.as_ref())
        {
            forced.as_str()
        } else {
            ctx.proton_path.as_deref()
                .filter(|p| !p.is_empty())
                .ok_or_else(|| LaunchError::new(LaunchErrorKind::Environment, "proton path is required for Windows launch"))?
        };
        let active_runner = crate::utils::resolve_runner(proton, &library_root);

        let mut spec = CommandSpec::default();

        // Build the base command (handles 'proton run' wrapper and directory resolution)
        let base_cmd = crate::utils::build_runner_command(&active_runner)
            .map_err(|e| LaunchError::new(LaunchErrorKind::Runner, format!("Invalid Compatibility Layer path: {}", active_runner.display())).with_source(e))?;
        spec.program = base_cmd.get_program().into();
        spec.args = base_cmd.get_args().map(|s| s.to_string_lossy().to_string()).collect();

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

        println!("--- RUNNER LAUNCH ---");
        println!("Program: {:?}", cmd.get_program());
        println!("Args: {:?}", cmd.get_args().collect::<Vec<_>>());
        println!("Working Dir: {:?}", cmd.get_current_dir());
        println!("-------------------------");

        cmd.spawn().map_err(|e| LaunchError::new(LaunchErrorKind::Process, "failed to spawn runner process").with_source(anyhow!(e)))
    }
}

