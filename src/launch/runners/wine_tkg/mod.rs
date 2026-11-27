use crate::launch::error::{LaunchError, LaunchErrorKind};
use crate::launch::runners::{CommandSpec, LaunchContext, Runner};
use anyhow::anyhow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

mod env;
mod prefix;
pub mod steam_mode;

use steam_mode::{SteamMode, resolve_steam_mode};

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
        Ok(Some(status)) => ReadinessGrace::ExitedEarly {
            code: status.code(),
        },
        // Still running, or the wait itself errored (treat as still-alive: the
        // subsequent launch path will surface any real problem).
        _ => ReadinessGrace::Ready,
    }
}

/// Steam integration facts, derived per phase.
struct SteamLayout {
    library_root: PathBuf,
    steam_mode: SteamMode,
    steam_mode_source: &'static str,
    steam_prefix_mode: crate::core::models::SteamPrefixMode,
    compat_data_path: PathBuf,
    game_prefix: PathBuf,
}

impl SteamLayout {
    /// Derive the layout from `ctx`.
    fn derive(ctx: &LaunchContext) -> Self {
        let library_root = PathBuf::from(&ctx.launcher_config.steam_library_path);
        let (steam_mode, steam_mode_source) = resolve_steam_mode(ctx);
        let custom_prefix = ctx.launcher_config.prefix_dir(ctx.app.app_id);
        // Steam must share the custom prefix.
        let steam_prefix_mode = if custom_prefix.is_some() {
            crate::core::models::SteamPrefixMode::PerGame
        } else {
            ctx.user_config
                .as_ref()
                .map(|c| c.steam_prefix_mode.clone())
                .unwrap_or_else(|| ctx.launcher_config.steam_prefix_mode.clone())
        };
        // Only Shared+InWine consults the master prefix.
        let master_wine_prefix = if steam_mode == SteamMode::InWineRuntime {
            crate::core::steam_host::get_master_steam_config().wine_prefix
        } else {
            PathBuf::new()
        };
        // Compatdata follows the game's library.
        let game_library = crate::core::paths::game_library_root(
            &ctx.launcher_config,
            ctx.app.app_id,
            ctx.app.install_path.as_deref().map(std::path::Path::new),
        );
        let compat_data_path = match custom_prefix {
            Some(dir) => dir.to_path_buf(),
            None => game_compat_data_path(
                &game_library,
                ctx.app.app_id,
                steam_mode,
                steam_prefix_mode.clone(),
                &master_wine_prefix,
            ),
        };
        let game_prefix = effective_game_prefix(ctx);
        Self {
            library_root,
            steam_mode,
            steam_mode_source,
            steam_prefix_mode,
            compat_data_path,
            game_prefix,
        }
    }
}

#[async_trait::async_trait]
impl Runner for WineTkgRunner {
    fn name(&self) -> &str {
        "Wine-TKG"
    }
    async fn prepare_prefix(&self, ctx: &LaunchContext) -> std::result::Result<(), LaunchError> {
        prefix::prepare_prefix(ctx).await
    }

    async fn build_env(
        &self,
        ctx: &LaunchContext,
    ) -> std::result::Result<HashMap<String, String>, LaunchError> {
        env::build_env(ctx).await
    }

    async fn build_command(
        &self,
        ctx: &LaunchContext,
    ) -> std::result::Result<CommandSpec, LaunchError> {
        let library_root = PathBuf::from(&ctx.launcher_config.steam_library_path);

        let proton = resolve_proton_required(ctx)?;
        let active_runner = crate::core::runner::resolve_runner(proton, &library_root);

        // Classify the GAME runner to record the launch route. A real Proton tree is
        // launched via its `proton run` script (protonfixes apply naturally); plain
        // Wine / wine-tkg launch via bare wine and rely on the data-driven fixup layer
        // (merged in build_env). In every case the game binary is a DIRECT spawn arg
        // to the runner — never a `steam://run/<appid>` or `-applaunch` handoff.
        match crate::core::runner::classify_runner(&active_runner) {
            crate::core::runner::RunnerKind::Proton => {
                tracing::info!(
                    "Game runner classified as Proton: launching via `proton run` (protonfixes active)"
                );
            }
            crate::core::runner::RunnerKind::WineTkg
            | crate::core::runner::RunnerKind::PlainWine => {
                tracing::info!(
                    fixup_env = ctx.game_fixups.env.len(),
                    fixup_dll = ctx.game_fixups.dll_overrides.len(),
                    "Game runner classified as bare Wine: launching via wine + fixup layer"
                );
            }
            crate::core::runner::RunnerKind::Unknown => {
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
            let base_cmd =
                crate::core::runner::build_runner_command(&active_runner).map_err(|e| {
                    LaunchError::new(
                        LaunchErrorKind::Runner,
                        format!(
                            "Invalid Compatibility Layer path: {}",
                            active_runner.display()
                        ),
                    )
                    .with_source(e)
                })?;
            spec.program = base_cmd.get_program().into();
            spec.args = base_cmd
                .get_args()
                .map(|s| s.to_string_lossy().to_string())
                .collect();
        }

        let game = ctx.launch_info.command(ctx.install_dir()?);

        spec.cwd = Some(game.cwd);
        spec.args.push(game.program.to_string_lossy().to_string());
        spec.args.extend(game.args);

        spec.env = self.build_env(ctx).await?;

        if use_umu {
            // umu-run needs GAMEID (the Steam AppID) and PROTONPATH (the absolute
            // directory of the resolved Proton tool) to select and run the game.
            let proton_root = crate::core::runner::derive_runner_root(&active_runner);
            spec.env
                .insert("GAMEID".to_string(), ctx.app.app_id.to_string());
            spec.env.insert(
                "PROTONPATH".to_string(),
                proton_root.to_string_lossy().to_string(),
            );
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
                cmd.stderr(crate::logging::relay::child_stdio());
            }
        } else {
            cmd.stderr(crate::logging::relay::child_stdio());
        }

        cmd.stdout(crate::logging::relay::child_stdio());

        tracing::debug!(
            program = ?cmd.get_program(),
            args = ?cmd.get_args().collect::<Vec<_>>(),
            working_dir = ?cmd.get_current_dir(),
            "runner launch",
        );

        cmd.spawn().map(crate::logging::relay::attach).map_err(|e| {
            LaunchError::new(LaunchErrorKind::Process, "failed to spawn runner process")
                .with_source(anyhow!(e))
        })
    }
}

/// Build the effective Wine prefix for the game from the launch context.
///
/// Wraps this game's per-app config (if any) in a single-entry `UserConfigStore`
/// and resolves the prefix the same way the launcher does globally.
fn effective_game_prefix(ctx: &LaunchContext) -> PathBuf {
    crate::core::paths::wineprefix_for_game(
        &ctx.launcher_config,
        ctx.app.app_id,
        ctx.user_config.as_ref(),
        ctx.app.install_path.as_deref().map(std::path::Path::new),
    )
}

/// Resolve STEAM_COMPAT_DATA_PATH for the game.
fn game_compat_data_path(
    library_root: &Path,
    app_id: u32,
    steam_mode: SteamMode,
    steam_prefix_mode: crate::core::models::SteamPrefixMode,
    master_wine_prefix: &Path,
) -> PathBuf {
    use crate::core::models::SteamPrefixMode;
    if steam_mode == SteamMode::InWineRuntime
        && steam_prefix_mode == SteamPrefixMode::Shared
        && let Some(parent) = master_wine_prefix.parent()
    {
        // Shareable only for proton pfx layout.
        if parent.join("pfx") == master_wine_prefix {
            return parent.to_path_buf();
        }
    }
    crate::core::paths::compat_data_dir(library_root, app_id)
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
        ctx.proton_path
            .as_deref()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| {
                LaunchError::new(
                    LaunchErrorKind::Environment,
                    "proton path is required for Windows launch",
                )
            })
    }
}
#[cfg(test)]
mod tests;
