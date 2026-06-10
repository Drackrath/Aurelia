use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError, LaunchErrorKind};

pub struct ResolveGameStage;

#[async_trait]
impl PipelineStage for ResolveGameStage {
    fn name(&self) -> &str { "ResolveGame" }
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        if ctx.app.is_none() {
            // In the future, we might resolve the app here if only app_id is provided
            return Err(LaunchError::new(LaunchErrorKind::Validation, "App context missing in ResolveGameStage")
                .with_context("app_id", ctx.app_id.to_string()));
        }
        Ok(())
    }
}
