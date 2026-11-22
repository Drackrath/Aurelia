//! Game environment for Wine launches.

use super::steam_mode::SteamMode;
use super::{SteamLayout, forced_proton};
use crate::core::components::RunnerComponents;
use crate::core::models::GraphicsLayerConfig;
use crate::launch::error::LaunchError;
use crate::launch::runners::LaunchContext;
use crate::steam_client::SteamClient;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Graphics provider choices for one launch.
#[derive(Clone, Copy)]
struct GraphicsFlags {
    effective_dxvk: bool,
    force_builtin_d3d: bool,
    strict_dxvk: bool,
    effective_vkd3d_proton: bool,
    effective_vkd3d: bool,
}

/// Full environment for the game process.
pub(super) async fn build_env(
    ctx: &LaunchContext,
) -> std::result::Result<HashMap<String, String>, LaunchError> {
    let mut env = HashMap::new();
    let app_id_str = ctx.app.app_id.to_string();

    let layout = SteamLayout::derive(ctx);
    let SteamLayout {
        ref library_root,
        steam_mode,
        ref compat_data_path,
        game_prefix: ref effective_game_prefix,
        ..
    } = layout;

    insert_game_identity(
        &mut env,
        app_id_str,
        effective_game_prefix,
        compat_data_path,
    )
    .await;

    let glc = ctx
        .user_config
        .as_ref()
        .map(|c| c.graphics_layers.clone())
        .unwrap_or_default();
    let no_overlay = ctx
        .user_config
        .as_ref()
        .map(|c| c.steam_launch_config.no_overlay)
        .unwrap_or(true);

    let (_install_dir, _executable, game_working_dir) = ctx.game_paths()?;

    // Resolve proton version for component detection and DLL path building
    let proton = forced_proton(ctx)
        .or_else(|| ctx.proton_path.as_deref().filter(|p| !p.is_empty()))
        .unwrap_or("wine");

    let active_runner_path = crate::core::runner::resolve_runner(proton, library_root);
    let components = crate::core::components::detect_runner_components(
        &active_runner_path,
        Some(effective_game_prefix),
    );

    let flags = resolve_graphics(ctx, &glc, &components);

    // Resolve the Steam-integration mode once for the whole env build. Only the
    // host-bridge mode leaves Steam's client DLLs at Proton's defaults (so
    // `lsteamclient` bridges to the host client). The in-Wine runtime and
    // standalone modes both want the native/neutralised set (`steamclient=n`, …)
    // so the game loads the in-Wine `steamclient.dll` (or none at all).
    let host_bridge = steam_mode == SteamMode::HostBridge;

    let use_symlinks = glc.use_symlinks_in_prefix;
    let dll_overrides = wine_dll_overrides(
        ctx,
        flags,
        no_overlay,
        &game_working_dir,
        host_bridge,
        use_symlinks,
    );
    tracing::info!("Final WINEDLLOVERRIDES: {}", dll_overrides);
    env.insert("WINEDLLOVERRIDES".to_string(), dll_overrides);

    insert_wine_dll_paths(&mut env, ctx, flags, use_symlinks, proton, library_root);
    expose_steam_client(&mut env, ctx, &layout)?;
    insert_display_env(&mut env);
    insert_gpu_preference(&mut env, ctx);
    apply_env_overrides(&mut env, ctx, flags);
    insert_mangohud(&mut env, ctx);

    env.insert(
        "WINEDEBUG".to_string(),
        "err+all,warn+module,warn+loaddll".to_string(),
    );

    let log_dir = crate::core::config::config_dir()
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("logs");
    let log_path = log_dir.join(format!("wine_{}.log", ctx.app.app_id));
    env.insert(
        "WINE_LOG_OUTPUT".to_string(),
        log_path.to_string_lossy().to_string(),
    );

    Ok(env)
}

/// Steam ids and prefix paths.
async fn insert_game_identity(
    env: &mut HashMap<String, String>,
    app_id_str: String,
    effective_game_prefix: &Path,
    compat_data_path: &Path,
) {
    env.insert("SteamAppId".to_string(), app_id_str.clone());
    env.insert("SteamGameId".to_string(), app_id_str.clone());
    env.insert("STEAM_COMPAT_APP_ID".to_string(), app_id_str);
    env.insert(
        "WINEPREFIX".to_string(),
        effective_game_prefix.to_string_lossy().to_string(),
    );
    env.insert(
        "STEAM_COMPAT_DATA_PATH".to_string(),
        compat_data_path.to_string_lossy().to_string(),
    );

    // Add user identity context if available
    if let Ok(session) = crate::core::config::load_session().await {
        if let Some(steam_id) = session.steam_id {
            env.insert("SteamUser".to_string(), steam_id.to_string());
        }
        if let Some(account_name) = session.account_name {
            env.insert("SteamAppUser".to_string(), account_name);
        }
    }
}

/// DX8-12 and NVAPI policy for this game.
fn resolve_graphics(
    ctx: &LaunchContext,
    glc: &GraphicsLayerConfig,
    components: &RunnerComponents,
) -> GraphicsFlags {
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
    let nvapi_enabled_cfg = ctx
        .user_config
        .as_ref()
        .map(|c| c.graphics_layers.nvapi_enabled)
        .unwrap_or(true);
    let nvapi_active = components.nvapi.is_some() && nvapi_enabled_cfg;
    if nvapi_active {
        tracing::info!("NVAPI component detected and enabled, will be exposed to game");
    } else if components.nvapi.is_some() {
        tracing::info!("NVAPI component detected but disabled by per-game settings");
    }

    GraphicsFlags {
        effective_dxvk,
        force_builtin_d3d,
        strict_dxvk,
        effective_vkd3d_proton,
        effective_vkd3d,
    }
}

/// WINEDLLOVERRIDES from policy, resolver and fixups.
fn wine_dll_overrides(
    ctx: &LaunchContext,
    flags: GraphicsFlags,
    no_overlay: bool,
    game_working_dir: &Path,
    host_bridge: bool,
    use_symlinks: bool,
) -> String {
    let GraphicsFlags {
        effective_dxvk,
        force_builtin_d3d,
        strict_dxvk,
        effective_vkd3d_proton,
        effective_vkd3d,
    } = flags;
    let mut dll_overrides = crate::core::dll_overrides::build_dll_overrides(
        effective_dxvk,
        effective_vkd3d_proton,
        effective_vkd3d,
        no_overlay,
        force_builtin_d3d,
        Some(game_working_dir),
        strict_dxvk,
        host_bridge,
    );

    // Enhance overrides with resolved DLL providers
    for res in &ctx.dll_resolutions {
        if res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::GameLocal
            || (res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::Custom
                && !use_symlinks)
            || (res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::Runner
                && res.name.contains("nvapi"))
        {
            // Native must win; replace by name.
            let already_native =
                crate::core::dll_overrides::dll_override_mode(&dll_overrides, &res.name)
                    .is_some_and(|m| m.starts_with('n'));
            if !already_native {
                tracing::info!(
                    "Adding native override for resolved DLL: {} (provider: {:?})",
                    res.name,
                    res.chosen_provider
                );
                dll_overrides = crate::core::dll_overrides::merge_dll_overrides(
                    &dll_overrides,
                    &[(res.name.clone(), "n".to_string())],
                );
            }
        } else if res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::Internal
        {
            tracing::info!(
                "Resolved DLL {} is handled internally (alias), skipping explicit override",
                res.name
            );
        }
    }

    // Merge auto-fixup DLL overrides from the per-game registry. Existing
    // entries (policy/provider-derived, i.e. the explicit resolution) WIN: a
    // fixup is only appended for a DLL not already present in the override string.
    if !ctx.game_fixups.dll_overrides.is_empty() {
        let existing_names = crate::core::dll_overrides::dll_override_names(&dll_overrides);
        for (dll, mode) in &ctx.game_fixups.dll_overrides {
            let key = crate::core::dll_overrides::normalize_dll_name(dll);
            if existing_names.contains(&key) {
                tracing::info!(
                    "Skipping fixup DLL override {}={} (explicit override already present)",
                    dll,
                    mode
                );
                continue;
            }
            tracing::info!("Applying fixup DLL override: {}={}", dll, mode);
            dll_overrides = crate::core::dll_overrides::merge_dll_overrides(
                &dll_overrides,
                &[(dll.clone(), mode.clone())],
            );
        }
    }

    dll_overrides
}

/// WINEDLLPATH and WINEPATH for runner DLLs.
fn insert_wine_dll_paths(
    env: &mut HashMap<String, String>,
    ctx: &LaunchContext,
    flags: GraphicsFlags,
    use_symlinks: bool,
    proton: &str,
    library_root: &Path,
) {
    let GraphicsFlags {
        effective_dxvk,
        effective_vkd3d_proton,
        effective_vkd3d,
        ..
    } = flags;
    // Track effective state for diagnostics (HACK: should ideally be done in a separate stage)
    // This is safe because WineTkgRunner is currently the only one implementing this logic.
    // We'll see if we can move it to PipelineContext later.

    // Translate Runner-resolved DLL paths into WINEDLLPATH so Wine can
    // actually find the bundled DLLs (VKD3D-Proton, DXVK, etc.) in the runner.
    // WITHOUT THIS, d3d12=n,b finds whatever is in the prefix's system32 instead.
    // CONSERVATIVE: only include paths for DLLs that are actually requested to be native.
    let mut wine_dll_dirs: Vec<String> = Vec::new();

    for res in &ctx.dll_resolutions {
        if (res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::Runner
            || res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::Custom)
            && !use_symlinks
        {
            // Check if this DLL is actually selected for use by the current policy/overrides
            let name = res.name.to_lowercase();
            let is_dxvk_dll = matches!(
                name.as_str(),
                "d3d8" | "d3d9" | "d3d10" | "d3d10_1" | "d3d10core" | "d3d11" | "dxgi"
            );
            let is_d3d12_dll = matches!(
                name.as_str(),
                "d3d12" | "d3d12core" | "libvkd3d-1" | "libvkd3d-shader-1"
            );

            let is_nvapi_dll = matches!(name.as_str(), "nvapi" | "nvapi64" | "nvofapi64");
            let selected = (is_dxvk_dll && effective_dxvk)
                || (is_d3d12_dll && (effective_vkd3d_proton || effective_vkd3d))
                || is_nvapi_dll;

            if !selected {
                continue;
            }

            if let Some(path) = &res.chosen_path
                && let Some(parent) = path.parent()
            {
                push_dll_dir_once(&mut wine_dll_dirs, parent);

                // For Wine-TKG and similar layouts, we must ensure both 64-bit and 32-bit
                // architecture folders are in WINEDLLPATH if they exist, so that both
                // architectures of a game find their respective native DLLs.
                let folder_name = parent.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let sibling_arch = match folder_name {
                    "x86_64-windows" => Some("i386-windows"),
                    "i386-windows" => Some("x86_64-windows"),
                    _ => None,
                };
                if let Some(arch) = sibling_arch {
                    let sibling = parent.parent().unwrap().join(arch);
                    if sibling.exists() {
                        push_dll_dir_once(&mut wine_dll_dirs, &sibling);
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
    let active_runner = crate::core::runner::resolve_runner(proton, library_root);
    let runner_root = crate::core::runner::derive_runner_root(&active_runner);
    for lib_sub in crate::compat::proton::UNIFIED_LIB_SUBDIRS {
        let p = runner_root.join(lib_sub);
        if p.exists() {
            push_dll_dir_once(&mut wine_dll_dirs, &p);

            // Ensure architecture-specific subdirectories are also in WINEDLLPATH.
            // This is critical for PE-based runners where Wine expects DLLs in
            // x86_64-windows/i386-windows folders, and for WOW64 builds whose unix
            // bridge libs live under x86_64-unix/i386-unix.
            for arch in crate::compat::proton::ARCH_SUBDIRS {
                let arch_p = p.join(arch);
                if arch_p.exists() {
                    push_dll_dir_once(&mut wine_dll_dirs, &arch_p);
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
}

/// STEAM_COMPAT_CLIENT_INSTALL_PATH for the Steam mode.
fn expose_steam_client(
    env: &mut HashMap<String, String>,
    ctx: &LaunchContext,
    layout: &SteamLayout,
) -> std::result::Result<(), LaunchError> {
    let steam_mode = layout.steam_mode;
    let effective_game_prefix = &layout.game_prefix;
    // Expose the Steam client install path the game's DRM/Steamworks bootstrap
    // reads (`STEAM_COMPAT_CLIENT_INSTALL_PATH`), matching the resolved mode:
    //   - HostBridge    → the host Steam install (Proton's lsteamclient bridges).
    //   - InWineRuntime → the in-Wine Steam runtime (real steamclient.dll +
    //                     the steam.exe prepare_prefix starts) — provides DRM
    //                     with no host Steam (issue #3).
    //   - Standalone    → the fake-Steam trap (no DRM).
    let fake_trap = |env: &mut HashMap<String, String>| -> std::result::Result<(), LaunchError> {
        let fake_env = crate::launch::runners::insert_fake_steam_trap(env)?;
        ctx.with_verification(|v| {
            v.steam_client_install_path_exposed_to_game =
                Some(fake_env.to_string_lossy().to_string());
            v.steam_client_install_path_source = Some("fake_trap".to_string());
        });
        Ok(())
    };
    let expose = |env: &mut HashMap<String, String>, path: PathBuf, source: &str| {
        env.insert(
            "STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string(),
            path.to_string_lossy().to_string(),
        );
        ctx.with_verification(|v| {
            v.steam_client_install_path_exposed_to_game = Some(path.to_string_lossy().to_string());
            v.steam_client_install_path_source = Some(source.to_string());
        });
    };

    match steam_mode {
        SteamMode::HostBridge => match crate::core::steam_host::host_steam_client_path() {
            Some(path) => expose(env, path, "real_host"),
            // resolve_steam_mode only picks HostBridge when host Steam exists, so
            // this is defensive (a Steam install vanishing mid-launch).
            None => fake_trap(env)?,
        },
        SteamMode::InWineRuntime => {
            let steam_cfg = crate::core::steam_host::get_master_steam_config();
            let steam_client_path = match layout.steam_prefix_mode.clone() {
                crate::core::models::SteamPrefixMode::Shared => steam_cfg
                    .steam_exe
                    .as_ref()
                    .and_then(|e| e.parent().map(|p| p.to_path_buf())),
                crate::core::models::SteamPrefixMode::PerGame => {
                    Some(effective_game_prefix.join("drive_c/Program Files (x86)/Steam"))
                }
            };

            match steam_client_path {
                Some(path) => expose(env, path, "real"),
                None => fake_trap(env)?,
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
            fake_trap(env)?;
        }
    }
    Ok(())
}

/// Forward display and X auth variables.
fn insert_display_env(env: &mut HashMap<String, String>) {
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
}

/// PRIME offload for the preferred GPU.
fn insert_gpu_preference(env: &mut HashMap<String, String>, ctx: &LaunchContext) {
    // Apply GPU preference if specified. CONSERVATIVE: No forced offload if unset.
    if let Some(gpu_pref) = ctx
        .user_config
        .as_ref()
        .and_then(|c| c.gpu_preference.as_ref())
    {
        let available_gpus = crate::core::sysinfo::list_available_gpus();
        if let Some(gpu) = available_gpus.iter().find(|g| &g.name == gpu_pref) {
            if gpu.name.contains("NVIDIA") {
                env.insert("__NV_PRIME_RENDER_OFFLOAD".to_string(), "1".to_string());
                env.insert(
                    "__NV_PRIME_RENDER_OFFLOAD_PROVIDER".to_string(),
                    "NVIDIA-G0".to_string(),
                );
                env.insert(
                    "__VK_LAYER_NV_optimus".to_string(),
                    "NVIDIA_only".to_string(),
                );
                env.insert(
                    "__GLX_VENDOR_LIBRARY_NAME".to_string(),
                    "nvidia".to_string(),
                );
            } else if gpu.name.contains("AMD")
                || gpu.name.contains("Intel")
                || gpu.name.contains("Unknown")
            {
                // Standard DRI_PRIME for non-NVIDIA discrete/specific GPUs
                // Try to find "cardN" and extract N
                static CARD_RE: std::sync::LazyLock<regex::Regex> =
                    std::sync::LazyLock::new(|| regex::Regex::new(r"card(\d+)").unwrap());
                if let Some(card_idx) = CARD_RE
                    .captures(&gpu.name)
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
}

/// Fixup env, then per-game env.
fn apply_env_overrides(
    env: &mut HashMap<String, String>,
    ctx: &LaunchContext,
    flags: GraphicsFlags,
) {
    let GraphicsFlags {
        effective_dxvk,
        effective_vkd3d_proton,
        effective_vkd3d,
        ..
    } = flags;
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
            if key == "WINEDLLOVERRIDES" {
                // Fill gaps; computed entries win.
                let computed = env.get(key).cloned().unwrap_or_default();
                let merged = crate::core::dll_overrides::fill_dll_overrides(
                    &computed,
                    &crate::core::dll_overrides::dll_override_entries(val),
                );
                tracing::info!("Applying fixup DLL overrides: {}", merged);
                env.insert(key.clone(), merged);
                continue;
            }
            if user_has(key) {
                tracing::info!(
                    "Skipping fixup env {} (explicit per-game value present)",
                    key
                );
                continue;
            }
            tracing::info!("Applying fixup env: {}={}", key, val);
            env.insert(key.clone(), val.clone());
        }
    }

    if let Some(config) = &ctx.user_config {
        for (key, val) in &config.env_variables {
            if key == "WINEDLLOVERRIDES" {
                // User wins per DLL only.
                let computed = env.get(key).cloned().unwrap_or_default();
                let merged = crate::core::dll_overrides::merge_dll_overrides(
                    &computed,
                    &crate::core::dll_overrides::dll_override_entries(val),
                );
                tracing::info!("Final WINEDLLOVERRIDES (with per-game env): {}", merged);
                env.insert(key.clone(), merged);
                continue;
            }
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
}

/// Preload MangoHud when requested.
fn insert_mangohud(env: &mut HashMap<String, String>, ctx: &LaunchContext) {
    let wants_mangohud = ctx
        .user_config
        .as_ref()
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
                tracing::warn!("MangoHud requested but libMangoHud.so not found — skipping");
            }
        }
    }
}

/// Push dir if not already present.
fn push_dll_dir_once(dirs: &mut Vec<String>, path: &Path) {
    let s = path.to_string_lossy().to_string();
    if !dirs.contains(&s) {
        dirs.push(s);
    }
}
