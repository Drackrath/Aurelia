use super::super::*;
use crate::launch::context::PipelineContext;
use crate::launch::error::{LaunchError, LaunchErrorKind};
use crate::logging::{EventLogger, LaunchSession};
use async_trait::async_trait;
use tempfile::tempdir;

struct SuccessStage(&'static str);
#[async_trait]
impl PipelineStage for SuccessStage {
    fn name(&self) -> &str {
        self.0
    }
    async fn execute(&self, _ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        Ok(())
    }
}

struct FailStage(&'static str);
#[async_trait]
impl PipelineStage for FailStage {
    fn name(&self) -> &str {
        self.0
    }
    async fn execute(&self, _ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        Err(LaunchError::new(LaunchErrorKind::Unknown, "failure"))
    }
}

#[tokio::test]
async fn test_pipeline_order_and_success() {
    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(SuccessStage("stage1")));
    pipeline.add_stage(Box::new(SuccessStage("stage2")));

    let mut ctx = PipelineContext::new(0);
    assert!(pipeline.run(&mut ctx).await.is_ok());
}

#[tokio::test]
async fn test_pipeline_short_circuit() {
    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(FailStage("stage1")));
    pipeline.add_stage(Box::new(SuccessStage("stage2")));

    let mut ctx = PipelineContext::new(0);
    let res = pipeline.run(&mut ctx).await;

    assert!(res.is_err());
    let err = res.unwrap_err();
    assert_eq!(err.stage_name, "stage1");
}

#[tokio::test]
async fn test_pipeline_returned_error_context() {
    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(SuccessStage("stage1")));
    pipeline.add_stage(Box::new(FailStage("stage2")));

    let mut ctx = PipelineContext::new(0);
    let res = pipeline.run(&mut ctx).await;

    assert!(res.is_err());
    let err = res.unwrap_err();
    assert_eq!(err.stage_name, "stage2");
    assert!(err.inner.to_string().contains("failure"));
}

#[tokio::test]
async fn test_pipeline_logging() {
    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(SuccessStage("test_stage")));

    let tmp = tempdir().unwrap();
    let session = LaunchSession::new(tmp.path());
    let logger = EventLogger::new(&session).unwrap();

    let mut ctx = PipelineContext::new(123);
    ctx.logger = Some(logger);

    pipeline.run(&mut ctx).await.unwrap();

    let content = std::fs::read_to_string(session.event_log_path()).unwrap();
    assert!(content.contains("launch_start"));
    assert!(content.contains("stage_start"));
    assert!(content.contains("test_stage"));
    assert!(content.contains("stage_success"));
    assert!(content.contains("launch_end"));
}

#[tokio::test]
async fn test_pipeline_structured_error_logging() {
    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(FailStage("fail_stage")));

    let tmp = tempdir().unwrap();
    let session = LaunchSession::new(tmp.path());
    let logger = EventLogger::new(&session).unwrap();

    let mut ctx = PipelineContext::new(456);
    ctx.logger = Some(logger);

    let _ = pipeline.run(&mut ctx).await;

    let content = std::fs::read_to_string(session.event_log_path()).unwrap();
    assert!(content.contains("stage_failure"));
    assert!(content.contains("fail_stage"));
    assert!(content.contains("error_kind"));
    assert!(content.contains("Unknown"));
    assert!(content.contains("failure"));
}

#[test]
fn launch_options_apply_between_preflight_and_launch_script() {
    let pipeline = LaunchPipeline::with_default_stages();
    let names: Vec<&str> = pipeline.stages.iter().map(|s| s.name()).collect();
    let at = |name: &str| names.iter().position(|n| *n == name).unwrap();
    assert!(at("Preflight") < at("UserLaunchOptions"));
    assert!(at("UserLaunchOptions") < at("ApplyLaunchScript"));
}
