use super::*;
use tempfile::tempdir;

struct SuccessStage(&'static str);
#[async_trait]
impl PipelineStage for SuccessStage {
    fn name(&self) -> &str { self.0 }
    async fn execute(&self, _ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> { Ok(()) }
}

struct FailStage(&'static str);
#[async_trait]
impl PipelineStage for FailStage {
    fn name(&self) -> &str { self.0 }
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

#[test]
fn test_map_anyhow_error() {
    let err = anyhow::anyhow!("Permission denied: /tmp/pfx");
    let mapped = map_anyhow_error(err);
    assert_eq!(mapped.kind, LaunchErrorKind::Permission);

    let err = anyhow::anyhow!("file not found");
    let mapped = map_anyhow_error(err);
    assert_eq!(mapped.kind, LaunchErrorKind::GameData);

    let err = anyhow::anyhow!("random error");
    let mapped = map_anyhow_error(err);
    assert_eq!(mapped.kind, LaunchErrorKind::Unknown);
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

#[test]
fn test_map_io_error_not_found() {
    let err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
    let mapped = map_io_error(&err, None);
    assert_eq!(mapped.kind, LaunchErrorKind::GameData);
    assert!(mapped.message.contains("not found"));
    assert_eq!(mapped.context.get("io_kind").unwrap(), "NotFound");
}

#[test]
fn test_map_io_error_permission_denied() {
    let err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
    let mapped = map_io_error(&err, None);
    assert_eq!(mapped.kind, LaunchErrorKind::Permission);
    assert!(mapped.message.contains("Access denied"));
}

#[test]
fn test_map_io_error_lock_detected() {
    // Simulating a "Resource busy" or "locked" error which often maps to 'Other' or 'WouldBlock' in std::io
    let err = std::io::Error::new(std::io::ErrorKind::Other, "file is locked by another process");
    let mapped = map_io_error(&err, None);
    assert_eq!(mapped.kind, LaunchErrorKind::Process);
    assert!(mapped.message.contains("locked by another process"));
}

#[test]
fn test_map_io_error_with_explicit_dup_info() {
    let err = std::io::Error::new(std::io::ErrorKind::Other, "generic error");
    let info = DuplicateInstanceInfo {
        detected: true,
        source: "lockfile".to_string(),
    };
    let mapped = map_io_error(&err, Some(&info));
    assert_eq!(mapped.kind, LaunchErrorKind::Process);
    assert!(mapped.message.contains("Ensure no other instance is running"));
    assert_eq!(mapped.context.get("duplicate_instance_detected").unwrap(), "true");
    assert_eq!(mapped.context.get("duplicate_detection_source").unwrap(), "lockfile");
}

#[test]
fn test_detect_duplicate_instance_lockfile() {
    let tmp = tempfile::tempdir().unwrap();
    let lockfile = tmp.path().join(".aurelia_launch.lock");
    std::fs::write(&lockfile, "").unwrap();

    let mut ctx = PipelineContext::new(123);
    let mut spec = CommandSpec::default();
    spec.cwd = Some(tmp.path().to_path_buf());
    ctx.command_spec = Some(spec);

    let info = detect_duplicate_instance(&ctx);
    assert!(info.detected);
    assert_eq!(info.source, "lockfile");
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
