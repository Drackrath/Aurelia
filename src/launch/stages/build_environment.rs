use crate::launch::pipeline::{LaunchError, PipelineContext, PipelineStage};
use async_trait::async_trait;

pub struct BuildEnvironmentStage;

#[async_trait]
impl PipelineStage for BuildEnvironmentStage {
    fn name(&self) -> &str {
        "BuildEnvironment"
    }
    async fn execute(&self, _ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        // Currently build_env is called internally by build_command in Runner
        Ok(())
    }
}
