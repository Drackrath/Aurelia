pub mod overrides;
pub mod invariants;

use crate::launch::pipeline::PipelineContext;

pub trait LaunchValidator: Send + Sync {
    fn name(&self) -> &str;
    fn validate(&self, ctx: &mut PipelineContext);
}
