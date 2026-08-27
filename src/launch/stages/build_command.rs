use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError};

pub struct BuildCommandStage;

#[async_trait]
impl PipelineStage for BuildCommandStage {
    fn name(&self) -> &str { "BuildCommand" }
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        if ctx.runner.is_none() {
            return Ok(());
        }
        let runner_ctx = ctx.to_runner_context()?;
        if let Some(runner) = &ctx.runner {
            let spec = runner.build_command(&runner_ctx).await?;
            ctx.command_spec = Some(spec);
        }
        Ok(())
    }
}
