use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError, LaunchErrorKind};

use std::collections::HashMap;
use std::path::PathBuf;
use crate::infra::runners::{Runner, LaunchContext, CommandSpec};

pub struct ResolveComponentsStage;

pub struct NativeRunner;

#[async_trait::async_trait]
impl Runner for NativeRunner {
    fn name(&self) -> &str { "Native" }
    async fn prepare_prefix(&self, _ctx: &LaunchContext) -> std::result::Result<(), LaunchError> { Ok(()) }
    async fn build_env(&self, ctx: &LaunchContext) -> std::result::Result<HashMap<String, String>, LaunchError> {
        let mut env = HashMap::new();
        env.insert("SteamAppId".to_string(), ctx.app.app_id.to_string());
        if let Some(config) = &ctx.user_config {
            env.extend(config.env_variables.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        Ok(env)
    }
    async fn build_command(&self, ctx: &LaunchContext) -> std::result::Result<CommandSpec, LaunchError> {
        let install_path = ctx.app.install_path.as_ref()
            .ok_or_else(|| LaunchError::new(LaunchErrorKind::GameData, "Install path missing"))?;

        let exe_rel = ctx.launch_info.executable.replace('\\', "/");
        Ok(CommandSpec {
            program: PathBuf::from(install_path).join(&exe_rel),
            args: ctx.launch_info.arguments.split_whitespace().map(str::to_string).collect(),
            cwd: Some(PathBuf::from(install_path)),
            env: self.build_env(ctx).await?,
        })
    }
    fn launch(&self, spec: &CommandSpec) -> std::result::Result<std::process::Child, LaunchError> {
        let mut cmd = std::process::Command::new(&spec.program);
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd { cmd.current_dir(cwd); }
        cmd.envs(&spec.env);
        cmd.spawn().map_err(|e| LaunchError::new(LaunchErrorKind::Process, "Native launch failed").with_source(anyhow::anyhow!(e)))
    }
}

/// Whether this launch should be routed through the luxtorpeda plugin: either a one-off
/// `--native-engine` override, or the game is pinned to it while the feature is enabled.
fn wants_luxtorpeda(ctx: &PipelineContext) -> bool {
    if ctx.force_native_engine {
        return true;
    }
    let Some(config) = &ctx.launcher_config else { return false };
    config.luxtorpeda_enabled
        && config
            .game_configs
            .get(&ctx.app_id)
            .map(|g| g.runner == crate::config::GameRunner::Luxtorpeda)
            .unwrap_or(false)
}

/// Whether this launch should be wrapped through the umu-launcher plugin: either a
/// one-off `--umu` override, or the game is pinned to it while the feature is enabled.
fn wants_umu(ctx: &PipelineContext) -> bool {
    if ctx.force_umu {
        return true;
    }
    let Some(config) = &ctx.launcher_config else { return false };
    config.umu_enabled
        && config
            .game_configs
            .get(&ctx.app_id)
            .map(|g| g.runner == crate::config::GameRunner::Umu)
            .unwrap_or(false)
}

#[async_trait]
impl PipelineStage for ResolveComponentsStage {
    fn name(&self) -> &str { "ResolveComponents" }
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        use crate::infra::runners::{LuxtorpedaRunner, WineTkgRunner};
        use crate::steam_client::LaunchTarget;

        if ctx.runner.is_none() {
            // The luxtorpeda native-engine plugin is Linux-only. Route through it when the
            // launch was explicitly forced (`--native-engine`) or the game is pinned to it
            // and the feature is enabled.
            if cfg!(target_os = "linux") && wants_luxtorpeda(ctx) {
                ctx.runner = Some(Box::new(LuxtorpedaRunner) as Box<dyn Runner>);
                return Ok(());
            }

            // The umu-launcher plugin *wraps* Proton rather than replacing the runner:
            // it is Linux-only and, when active, we keep the normal Proton/Wine runner
            // but resolve the `umu-run` entry point (downloading on first use) so the
            // runner spawns the game through umu. A one-off `--umu` on a non-Linux host
            // is a hard error, matching the `--native-engine` guard.
            if ctx.force_umu && !cfg!(target_os = "linux") {
                return Err(LaunchError::new(
                    LaunchErrorKind::Validation,
                    "umu-launcher (`--umu`) is only available on Linux",
                ));
            }
            if cfg!(target_os = "linux") && wants_umu(ctx) {
                let custom = ctx
                    .launcher_config
                    .as_ref()
                    .and_then(|c| c.umu_path.clone());
                let custom_path = custom.as_deref().map(std::path::Path::new);
                let umu_run = crate::umu::ensure_installed(custom_path).await.map_err(|e| {
                    LaunchError::new(
                        LaunchErrorKind::Runner,
                        format!("failed to resolve the umu-launcher plugin: {e:#}"),
                    )
                    .with_source(e)
                })?;
                ctx.use_umu = true;
                ctx.umu_run = Some(umu_run);
            }

            let Some(info) = &ctx.launch_info else {
                return Err(LaunchError::new(LaunchErrorKind::Validation, "LaunchInfo missing in ResolveComponentsStage"));
            };
            ctx.runner = Some(match info.target {
                LaunchTarget::NativeLinux => Box::new(NativeRunner) as Box<dyn Runner>,
                LaunchTarget::WindowsProton => Box::new(WineTkgRunner),
            });
        }
        Ok(())
    }
}
