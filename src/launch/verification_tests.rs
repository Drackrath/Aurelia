use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use tempfile::tempdir;

use crate::launch::pipeline::{PipelineContext, LaunchPipeline, LaunchError, LaunchErrorKind};
use crate::infra::logging::{LaunchSession, EventLogger};
use crate::infra::runners::{Runner, CommandSpec, LaunchContext};

struct MockRunner {
    exit_immediately: bool,
}

#[async_trait]
impl Runner for MockRunner {
    fn name(&self) -> &str { "MockRunner" }
    async fn prepare_prefix(&self, _ctx: &LaunchContext) -> Result<(), LaunchError> { Ok(()) }
    async fn build_env(&self, _ctx: &LaunchContext) -> Result<HashMap<String, String>, LaunchError> { Ok(HashMap::new()) }
    async fn build_command(&self, _ctx: &LaunchContext) -> Result<CommandSpec, LaunchError> { Ok(CommandSpec::default()) }
    fn launch(&self, _spec: &CommandSpec) -> Result<Child, LaunchError> {
        let cmd = if self.exit_immediately {
            "exit 0"
        } else {
            "sleep 10"
        };
        Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| LaunchError::new(LaunchErrorKind::Process, e.to_string()))
    }
}


#[tokio::test]
async fn test_launch_verification_early_exit() {
    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(crate::launch::stages::spawn_process::SpawnProcessStage));

    let tmp = tempdir().unwrap();
    let session = LaunchSession::new(tmp.path());
    let logger = EventLogger::new(&session).unwrap();

    let mut ctx = PipelineContext::new(123);
    ctx.logger = Some(logger);
    ctx.session = Some(session);
    ctx.runner = Some(Box::new(MockRunner { exit_immediately: true }));
    ctx.command_spec = Some(CommandSpec {
        program: PathBuf::from("sh"),
        args: vec!["-c".to_string(), "exit 0".to_string()],
        ..Default::default()
    });

    let _ = pipeline.run(&mut ctx).await;

    assert_eq!(ctx.verification.status, "failed_after_spawn");
    assert!(ctx.verification.process_lifetime_ms.is_some());
    assert_eq!(ctx.verification.exit_code, Some(0));

    // Check summary for failure
    let summary_path = ctx.session.as_ref().unwrap().summary_path();
    let summary_content = std::fs::read_to_string(summary_path).unwrap();
    assert!(summary_content.contains("\"result\": \"Failure\""));
    assert!(summary_content.contains("\"status\": \"failed_after_spawn\""));
}

#[tokio::test]
async fn test_launch_verification_success() {
    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(crate::launch::stages::spawn_process::SpawnProcessStage));

    let tmp = tempdir().unwrap();
    let session = LaunchSession::new(tmp.path());
    let logger = EventLogger::new(&session).unwrap();

    let mut ctx = PipelineContext::new(123);
    ctx.logger = Some(logger);
    ctx.session = Some(session);
    ctx.runner = Some(Box::new(MockRunner { exit_immediately: false }));
    ctx.command_spec = Some(CommandSpec {
        program: PathBuf::from("sh"),
        args: vec!["-c".to_string(), "sleep 10".to_string()],
        ..Default::default()
    });

    let _ = pipeline.run(&mut ctx).await;

    assert_eq!(ctx.verification.status, "verified");
    assert!(ctx.verification.process_lifetime_ms.is_some());

    // Check summary for success
    let summary_path = ctx.session.as_ref().unwrap().summary_path();
    let summary_content = std::fs::read_to_string(summary_path).unwrap();
    assert!(summary_content.contains("\"result\": \"Success\""));
    assert!(summary_content.contains("\"status\": \"verified\""));

    // Cleanup the sleep process
    if let Some(mut child) = ctx.child.take() {
        let _ = child.kill();
    }
}
