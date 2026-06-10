use aurelia::launch::pipeline::{LaunchPipeline, PipelineContext, LaunchErrorKind, LaunchError};
use aurelia::infra::logging::{LaunchSession, LaunchResult, load_launch_summary};
use async_trait::async_trait;
use tempfile::tempdir;

struct MockSuccessStage(&'static str);
#[async_trait]
impl aurelia::launch::pipeline::PipelineStage for MockSuccessStage {
    fn name(&self) -> &str { self.0 }
    async fn execute(&self, _ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        Ok(())
    }
}

struct MockFailStage(&'static str);
#[async_trait]
impl aurelia::launch::pipeline::PipelineStage for MockFailStage {
    fn name(&self) -> &str { self.0 }
    async fn execute(&self, _ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        Err(LaunchError::new(LaunchErrorKind::Unknown, "forced failure"))
    }
}

#[tokio::test]
async fn test_summary_written_on_success() {
    let tmp = tempdir().unwrap();
    let session = LaunchSession::new(tmp.path());
    let mut ctx = PipelineContext::new(1);
    ctx.session = Some(session);

    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(MockSuccessStage("stage1")));

    pipeline.run(&mut ctx).await.unwrap();

    let summary = load_launch_summary(&ctx.session.as_ref().unwrap().log_dir).unwrap();
    assert_eq!(summary.result, LaunchResult::Success);
    assert!(summary.failing_stage.is_none());
    assert!(summary.total_duration_ms > 0);
    assert!(summary.stage_durations_ms.contains_key("stage1"));
    assert!(*summary.stage_durations_ms.get("stage1").unwrap() >= 10);
}

#[tokio::test]
async fn test_summary_written_on_failure() {
    let tmp = tempdir().unwrap();
    let session = LaunchSession::new(tmp.path());
    let mut ctx = PipelineContext::new(2);
    ctx.session = Some(session);

    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(MockFailStage("fail_stage")));

    let _ = pipeline.run(&mut ctx).await;

    let summary = load_launch_summary(&ctx.session.as_ref().unwrap().log_dir).unwrap();
    assert_eq!(summary.result, LaunchResult::Failure);
    assert_eq!(summary.failing_stage, Some("fail_stage".to_string()));
}
