//! Luxtorpeda runner — routes a game through the optional native-engine plugin.
//!
//! Unlike [`super::WineTkgRunner`], this runner is deliberately thin: luxtorpeda owns the
//! native engine download, its own prefix/compat data, and its own (Godot) engine-picker
//! UI. Aurelia only has to install the plugin on demand and invoke it the way Steam invokes
//! a compatibility tool — `luxtorpeda run <game.exe>` with the standard `STEAM_COMPAT_*`
//! environment set. The luxtorpeda program is a separate process (GPL-2.0), never linked in.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::anyhow;

use crate::infra::runners::{CommandSpec, LaunchContext, Runner};
use crate::launch::pipeline::{LaunchError, LaunchErrorKind};

pub struct LuxtorpedaRunner;

#[async_trait::async_trait]
impl Runner for LuxtorpedaRunner {
    fn name(&self) -> &str {
        "Luxtorpeda"
    }

    async fn prepare_prefix(&self, ctx: &LaunchContext) -> Result<(), LaunchError> {
        // Use a configured external install if present, else download on first use.
        crate::compat::luxtorpeda::ensure_installed(custom_path(ctx).as_deref()).await.map_err(|e| {
            LaunchError::new(
                LaunchErrorKind::Environment,
                "failed to install the luxtorpeda plugin",
            )
            .with_source(e)
        })?;

        // Luxtorpeda expects a Steam compat-data directory to exist for the app.
        let compat = compat_data_path(ctx);
        std::fs::create_dir_all(&compat).map_err(|e| {
            LaunchError::new(
                LaunchErrorKind::Permission,
                format!("failed creating {}", compat.display()),
            )
            .with_source(anyhow!(e))
        })?;
        Ok(())
    }

    async fn build_env(&self, ctx: &LaunchContext) -> Result<HashMap<String, String>, LaunchError> {
        let mut env = HashMap::new();
        let app_id = ctx.app.app_id.to_string();

        env.insert("SteamAppId".to_string(), app_id.clone());
        env.insert("SteamGameId".to_string(), app_id.clone());
        env.insert("STEAM_COMPAT_APP_ID".to_string(), app_id);
        env.insert(
            "STEAM_COMPAT_DATA_PATH".to_string(),
            compat_data_path(ctx).to_string_lossy().to_string(),
        );

        // Luxtorpeda (like Proton) wants a Steam client install path. Reuse Aurelia's
        // fake-steam trap so it resolves without a running Steam client.
        crate::infra::runners::insert_fake_steam_trap(&mut env)?;

        // Pass through display/session so the engine picker and game can open a window.
        for var in ["DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"] {
            if let Ok(val) = std::env::var(var) {
                env.insert(var.to_string(), val);
            }
        }

        // User-supplied per-game environment overrides win.
        if let Some(config) = &ctx.user_config {
            for (key, val) in &config.env_variables {
                env.insert(key.clone(), val.clone());
            }
        }

        Ok(env)
    }

    async fn build_command(&self, ctx: &LaunchContext) -> Result<CommandSpec, LaunchError> {
        let entry = crate::compat::luxtorpeda::ensure_installed(custom_path(ctx).as_deref()).await.map_err(|e| {
            LaunchError::new(
                LaunchErrorKind::Environment,
                "failed to resolve the luxtorpeda entry point",
            )
            .with_source(e)
        })?;

        let (_install_dir, executable, game_working_dir) = ctx.game_paths()?;

        let mut args = vec!["run".to_string(), executable.to_string_lossy().to_string()];
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
                    .map(ToString::to_string),
            );
        }

        Ok(CommandSpec {
            program: entry,
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
        cmd.envs(&spec.env);
        cmd.stdout(std::process::Stdio::inherit());
        cmd.stderr(std::process::Stdio::inherit());

        tracing::debug!(
            program = ?cmd.get_program(),
            args = ?cmd.get_args().collect::<Vec<_>>(),
            "luxtorpeda launch",
        );

        cmd.spawn().map_err(|e| {
            LaunchError::new(LaunchErrorKind::Process, "failed to spawn luxtorpeda process")
                .with_source(anyhow!(e))
        })
    }
}

/// A configured external luxtorpeda install path, if any.
fn custom_path(ctx: &LaunchContext) -> Option<PathBuf> {
    ctx.launcher_config
        .luxtorpeda_path
        .as_deref()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
}

/// Steam compat-data directory for this app under the active library.
fn compat_data_path(ctx: &LaunchContext) -> PathBuf {
    crate::core::utils::compat_data_dir(
        Path::new(&ctx.launcher_config.steam_library_path),
        ctx.app.app_id,
    )
}
