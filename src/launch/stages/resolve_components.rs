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
            for (k, v) in &config.env_variables {
                env.insert(k.clone(), v.clone());
            }
        }
        Ok(env)
    }
    async fn build_command(&self, ctx: &LaunchContext) -> std::result::Result<CommandSpec, LaunchError> {
        let install_path = ctx.app.install_path.as_ref()
            .ok_or_else(|| LaunchError::new(LaunchErrorKind::GameData, "Install path missing"))?;

        let mut spec = CommandSpec::default();
        let exe_rel = ctx.launch_info.executable.replace('\\', "/");
        let executable = PathBuf::from(install_path).join(&exe_rel);

        spec.program = executable;
        spec.args = ctx.launch_info.arguments.split_whitespace().map(|s| s.to_string()).collect();
        spec.cwd = Some(PathBuf::from(install_path));
        spec.env = self.build_env(ctx).await?;

        Ok(spec)
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
            if let Some(info) = &ctx.launch_info {
                match info.target {
                    LaunchTarget::NativeLinux => {
                        ctx.runner = Some(Box::new(NativeRunner));
                    }
                    LaunchTarget::WindowsProton => {
                        ctx.runner = Some(Box::new(WineTkgRunner));
                    }
                }
            } else {
                 return Err(LaunchError::new(LaunchErrorKind::Validation, "LaunchInfo missing in ResolveComponentsStage"));
            }
        }
        Ok(())
    }
}
