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

#[async_trait]
impl PipelineStage for ResolveComponentsStage {
    fn name(&self) -> &str { "ResolveComponents" }
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        use crate::infra::runners::WineTkgRunner;
        use crate::steam_client::LaunchTarget;

        if ctx.runner.is_none() {
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
