pub mod invariants;
pub mod overrides;

use crate::launch::context::PipelineContext;

pub trait LaunchValidator: Send + Sync {
    fn name(&self) -> &str;
    fn validate(&self, ctx: &mut PipelineContext);
}
