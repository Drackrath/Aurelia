use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError, LaunchErrorKind};

pub struct BuildCommandStage;

#[async_trait]
impl PipelineStage for BuildCommandStage {
    fn name(&self) -> &str { "BuildCommand" }
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        use crate::infra::runners::LaunchContext;

        if let Some(runner) = &ctx.runner {
            let runner_ctx = LaunchContext {
                app: ctx.app.as_ref()
                    .ok_or_else(|| LaunchError::new(LaunchErrorKind::Validation, "app missing"))?.clone(),
                launch_info: ctx.launch_info.as_ref()
                    .ok_or_else(|| LaunchError::new(LaunchErrorKind::Validation, "launch_info missing"))?.clone(),
                launcher_config: ctx.launcher_config.as_ref()
                    .ok_or_else(|| LaunchError::new(LaunchErrorKind::Validation, "launcher_config missing"))?.clone(),
                user_config: ctx.user_config.clone(),
                proton_path: ctx.proton_path.clone(),
                target_architecture: ctx.target_architecture,
                dll_resolutions: ctx.dll_resolutions.clone(),
                verification_ptr: &mut ctx.verification as *mut _,
            };
            let spec = runner.build_command(&runner_ctx).await?;
            ctx.command_spec = Some(spec);
        }
        Ok(())
    }
}
