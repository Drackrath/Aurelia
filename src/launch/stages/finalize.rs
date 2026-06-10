use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError};

pub struct FinalizeStage;

#[async_trait]
impl PipelineStage for FinalizeStage {
    fn name(&self) -> &str { "Finalize" }
    async fn execute(&self, _ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        Ok(())
    }
}
