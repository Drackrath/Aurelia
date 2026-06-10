use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError};

pub struct BuildEnvironmentStage;

#[async_trait]
impl PipelineStage for BuildEnvironmentStage {
    fn name(&self) -> &str { "BuildEnvironment" }
    async fn execute(&self, _ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        // Currently build_env is called internally by build_command in Runner
        Ok(())
    }
}
