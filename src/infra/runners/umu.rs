//! umu-launcher runner — routes a Windows/Proton game through `umu-run`.
//!
//! [`umu-launcher`](https://github.com/Open-Wine-Components/umu-launcher) wraps the Steam
//! Linux Runtime (pressure-vessel / sniper container) + Proton + protonfixes. Unlike
//! [`super::WineTkgRunner`], Aurelia does **not** drive Proton directly here and does **not**
//! own DLL deployment: `umu-run` takes the game executable directly (no `proton run` wrapper)
//! and Proton/protonfixes manage DXVK/VKD3D and the runtime container. Aurelia only supplies
//! the prefix path, the Proton directory (`PROTONPATH`), and the umu/Steam identity env.
//!
//! This runner is additive and opt-in (`umu_enabled` / per-game [`GameRunner::Umu`] /
//! one-off `--umu`); `WineTkgRunner` remains the default and fallback.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use anyhow::anyhow;

use crate::infra::runners::wine_tkg::{resolve_game_paths, resolve_proton_required};
use crate::infra::runners::{CommandSpec, LaunchContext, Runner};
use crate::launch::pipeline::{LaunchError, LaunchErrorKind};

pub struct UmuRunner;

#[async_trait::async_trait]
impl Runner for UmuRunner {
    fn name(&self) -> &str {
        "umu"
    }

    async fn prepare_prefix(&self, ctx: &LaunchContext) -> Result<(), LaunchError> {
        // umu / pressure-vessel create and initialise the WINEPREFIX and compatdata
        // themselves. We do the bare minimum: ensure the compatdata parent directory
        // exists so umu can write `<compatdata>/<appid>/` into it. We deliberately do NOT
        // replicate WineTkgRunner's `pfx.lock` pre-creation or the in-prefix Windows-Steam
        // bootstrap — umu owns prefix setup.
        let compat_data_path = compat_data_path(ctx);
        if let Some(parent) = compat_data_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                LaunchError::new(
                    LaunchErrorKind::Permission,
                    format!("failed creating {}", parent.display()),
                )
                .with_source(anyhow!(e))
            })?;
        }
        Ok(())
    }

    async fn build_env(&self, ctx: &LaunchContext) -> Result<HashMap<String, String>, LaunchError> {
        let mut env = HashMap::new();
        let app_id_str = ctx.app.app_id.to_string();

        let compat_data_path = compat_data_path(ctx);
        // umu/Proton derive the real prefix as `STEAM_COMPAT_DATA_PATH/pfx`. Force the
        // per-game compatdata layout for WINEPREFIX so it always matches
        // STEAM_COMPAT_DATA_PATH below — never the shared master prefix that
        // `effective_game_prefix` would yield with the default config (which umu would
        // ignore anyway). This is independent of `use_shared_compat_data`; the umu runner
        // owns its own per-game prefix layout.
        let wine_prefix = compat_data_path.join("pfx");

        // --- Steam compatibility identity ---------------------------------------------
        // SteamAppId / STEAM_COMPAT_APP_ID are REQUIRED so stop_game's /proc/*/environ
        // sweep can find the re-parented Proton/game children launched under umu.
        env.insert("SteamAppId".to_string(), app_id_str.clone());
        env.insert("SteamGameId".to_string(), app_id_str.clone());
        env.insert("STEAM_COMPAT_APP_ID".to_string(), app_id_str.clone());
        env.insert(
            "WINEPREFIX".to_string(),
            wine_prefix.to_string_lossy().to_string(),
        );
        env.insert(
            "STEAM_COMPAT_DATA_PATH".to_string(),
            compat_data_path.to_string_lossy().to_string(),
        );

        if let Ok(session) = crate::config::load_session().await {
            if let Some(steam_id) = session.steam_id {
                env.insert("SteamUser".to_string(), steam_id.to_string());
            }
            if let Some(account_name) = session.account_name {
                env.insert("SteamAppUser".to_string(), account_name);
            }
        }

        // --- umu-specific env ----------------------------------------------------------
        // GAMEID is the protonfixes lookup key; for Steam titles use `umu-<appid>` with
        // STORE=steam so umu/protonfixes apply Steam's fix list.
        env.insert("GAMEID".to_string(), format!("umu-{}", app_id_str));
        env.insert("STORE".to_string(), "steam".to_string());
        env.insert("PROTON_VERB".to_string(), "waitforexitandrun".to_string());

        // PROTONPATH must be the Proton *directory* (the runner root), not the `proton`
        // script — umu wants the dir. Point it at Aurelia's resolved/managed Proton so umu
        // does not download its own.
        let library_root = PathBuf::from(&ctx.launcher_config.steam_library_path);
        let proton = resolve_proton_required(ctx)?;
        let active_runner = crate::utils::resolve_runner(proton, &library_root);
        let proton_root = crate::utils::derive_runner_root(&active_runner);
        env.insert(
            "PROTONPATH".to_string(),
            proton_root.to_string_lossy().to_string(),
        );

        // --- STEAM_COMPAT_CLIENT_INSTALL_PATH -----------------------------------------
        // Reuse WineTkgRunner's host-Steam resolution: real host client when steam_enabled,
        // else the fake-steam trap so umu/Proton's "is Steam installed" check is satisfied.
        let config_dir = crate::config::config_dir().map_err(|e| {
            LaunchError::new(LaunchErrorKind::Environment, "failed to get config dir").with_source(e)
        })?;
        let client_install_path = if ctx.steam_enabled {
            match crate::utils::host_steam_client_path() {
                Some(p) => p,
                None => {
                    tracing::warn!(
                        "steam-enabled umu launch requested but no host Steam install found; using standalone trap"
                    );
                    crate::utils::setup_fake_steam_trap(&config_dir).map_err(|e| {
                        LaunchError::new(
                            LaunchErrorKind::Permission,
                            "failed to setup fake steam trap",
                        )
                        .with_source(e)
                    })?
                }
            }
        } else {
            crate::utils::setup_fake_steam_trap(&config_dir).map_err(|e| {
                LaunchError::new(LaunchErrorKind::Permission, "failed to setup fake steam trap")
                    .with_source(e)
            })?
        };
        env.insert(
            "STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string(),
            client_install_path.to_string_lossy().to_string(),
        );

        // --- Display / session passthrough --------------------------------------------
        for var in ["DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "XAUTHORITY"] {
            if let Ok(val) = std::env::var(var) {
                env.insert(var.to_string(), val);
            }
        }

        // --- GPU preference (PRIME offload), mirroring WineTkgRunner -------------------
        if let Some(gpu_pref) = ctx.user_config.as_ref().and_then(|c| c.gpu_preference.as_ref()) {
            let available_gpus = crate::utils::list_available_gpus();
            if let Some(gpu) = available_gpus.iter().find(|g| &g.name == gpu_pref) {
                if gpu.name.contains("NVIDIA") {
                    env.insert("__NV_PRIME_RENDER_OFFLOAD".to_string(), "1".to_string());
                    env.insert(
                        "__NV_PRIME_RENDER_OFFLOAD_PROVIDER".to_string(),
                        "NVIDIA-G0".to_string(),
                    );
                    env.insert("__VK_LAYER_NV_optimus".to_string(), "NVIDIA_only".to_string());
                    env.insert("__GLX_VENDOR_LIBRARY_NAME".to_string(), "nvidia".to_string());
                } else if gpu.name.contains("AMD")
                    || gpu.name.contains("Intel")
                    || gpu.name.contains("Unknown")
                {
                    static CARD_RE: std::sync::LazyLock<regex::Regex> =
                        std::sync::LazyLock::new(|| regex::Regex::new(r"card(\d+)").unwrap());
                    if let Some(card_idx) = CARD_RE
                        .captures(&gpu.name)
                        .and_then(|caps| caps.get(1))
                        .and_then(|m| m.as_str().parse::<u32>().ok())
                    {
                        let dri_prime = if card_idx > 0 { "1" } else { "0" };
                        env.insert("DRI_PRIME".to_string(), dri_prime.to_string());
                    }
                }
            }
        }

        // --- MangoHud (LD_PRELOAD), mirroring WineTkgRunner ---------------------------
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
            if let Some(lib) = crate::steam_client::SteamClient::find_mangohud_lib() {
                let existing = std::env::var("LD_PRELOAD").unwrap_or_default();
                let new_preload = if existing.is_empty() {
                    lib.to_string_lossy().to_string()
                } else {
                    format!("{}:{}", lib.to_string_lossy(), existing)
                };
                env.insert("LD_PRELOAD".to_string(), new_preload);
                env.insert("MANGOHUD".to_string(), "1".to_string());
                env.insert("MANGOHUD_DLSYM".to_string(), "1".to_string());
            } else {
                println!("⚠️  MangoHud requested but libMangoHud.so not found — skipping");
            }
        }

        // --- User per-game env overrides win ------------------------------------------
        // NB: under umu, Aurelia deliberately does NOT set WINEDLLOVERRIDES / WINEDLLPATH —
        // Proton/protonfixes own DLL management. A user-supplied WINEDLLOVERRIDES is still
        // passed through additively here.
        if let Some(config) = &ctx.user_config {
            for (key, val) in &config.env_variables {
                env.insert(key.clone(), val.clone());
            }
        }

        // --- Logging: wire umu/Proton output into the same sink the pipeline scans -----
        env.insert("PROTON_LOG".to_string(), "1".to_string());
        let log_dir = crate::config::config_dir()
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
            .join("logs");
        let log_path = log_dir.join(format!("wine_{}.log", ctx.app.app_id));
        env.insert(
            "WINE_LOG_OUTPUT".to_string(),
            log_path.to_string_lossy().to_string(),
        );

        Ok(env)
    }

    async fn build_command(&self, ctx: &LaunchContext) -> Result<CommandSpec, LaunchError> {
        let (_install_dir, executable, game_working_dir) = resolve_game_paths(ctx)?;

        // umu-run takes the game executable directly — NO `run` sub-command.
        let mut args = vec![executable.to_string_lossy().to_string()];
        args.extend(
            ctx.launch_info
                .arguments
                .split_whitespace()
                .map(ToString::to_string),
        );
        if let Some(config) = &ctx.user_config {
            args.extend(
                config
                    .launch_options
                    .split_whitespace()
                    .filter(|a| *a != "-mangohud" && *a != "--mangohud")
                    .map(ToString::to_string),
            );
        }

        Ok(CommandSpec {
            // program = configured umu_path, else `umu-run` resolved on $PATH.
            program: resolve_umu_binary(ctx),
            args,
            cwd: Some(game_working_dir),
            env: self.build_env(ctx).await?,
        })
    }

    fn launch(&self, spec: &CommandSpec) -> Result<std::process::Child, LaunchError> {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        for (key, val) in &spec.env {
            cmd.env(key, val);
        }

        // Redirect stderr to the same WINE_LOG_OUTPUT sink WineTkgRunner uses so the
        // pipeline's launch-log scanning works unchanged.
        let log_path = spec.env.get("WINE_LOG_OUTPUT").map(PathBuf::from);
        if let Some(path) = log_path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
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
            "umu launch",
        );

        cmd.spawn().map_err(|e| {
            LaunchError::new(LaunchErrorKind::Process, "failed to spawn umu-run process")
                .with_source(anyhow!(e))
        })
    }
}

/// Steam compat-data directory (the parent of `pfx`) for this app under the active library.
fn compat_data_path(ctx: &LaunchContext) -> PathBuf {
    umu_compat_data_path(&ctx.launcher_config.steam_library_path, ctx.app.app_id)
}

/// The per-game compat-data directory umu uses for an app, independent of
/// `steam_prefix_mode` / `use_shared_compat_data` (umu always owns a per-game layout).
/// The actual `WINEPREFIX` is this path joined with `pfx`. Shared with the running-game
/// recorder in `steam_client::launch` so `aurelia stop`'s prefix sweep matches the prefix
/// umu actually used.
pub(crate) fn umu_compat_data_path(steam_library_path: &str, app_id: u32) -> PathBuf {
    PathBuf::from(steam_library_path)
        .join("steamapps")
        .join("compatdata")
        .join(app_id.to_string())
}

/// Resolve the `umu-run` binary: the configured `umu_path` when set, else `umu-run`
/// (looked up on `$PATH` at spawn time).
pub(crate) fn resolve_umu_binary(ctx: &LaunchContext) -> PathBuf {
    umu_binary(ctx.launcher_config.umu_path.as_deref())
}

/// Resolve the `umu-run` binary from an optional configured path.
pub fn umu_binary(configured: Option<&str>) -> PathBuf {
    match configured.filter(|p| !p.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from("umu-run"),
    }
}
