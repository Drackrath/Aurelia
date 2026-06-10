use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError, LaunchErrorKind};

pub struct ResolveProfileStage;

#[async_trait]
impl PipelineStage for ResolveProfileStage {
    fn name(&self) -> &str { "ResolveProfile" }
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        if ctx.launch_info.is_none() {
            return Err(LaunchError::new(LaunchErrorKind::Validation, "LaunchInfo missing in ResolveProfileStage")
                .with_context("app_id", ctx.app_id.to_string()));
        }
        Ok(())
    }
}
