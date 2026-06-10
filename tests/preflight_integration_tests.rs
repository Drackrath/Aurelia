use aurelia::launch::pipeline::{LaunchPipeline, PipelineContext};
use aurelia::launch::stages::preflight::PreflightStage;
use aurelia::infra::runners::CommandSpec;
use std::path::Path;

#[tokio::test]
async fn test_pipeline_preflight_prevents_spawn() {
    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(PreflightStage));
    // We add a dummy success stage that should NOT be reached if preflight fails
    struct SpawnShouldNotBeReached;
    #[async_trait::async_trait]
    impl aurelia::launch::pipeline::PipelineStage for SpawnShouldNotBeReached {
        fn name(&self) -> &str { "SpawnProcess" }
        async fn execute(&self, _ctx: &mut PipelineContext) -> Result<(), aurelia::launch::pipeline::LaunchError> {
            panic!("SpawnProcess stage reached but should have been prevented by Preflight!");
        }
    }
    pipeline.add_stage(Box::new(SpawnShouldNotBeReached));

    let mut ctx = PipelineContext::new(123);
    let mut spec = CommandSpec::default();
    spec.program = Path::new("/nonexistent/exe/that/fails/preflight").to_path_buf();
    ctx.command_spec = Some(spec);

    let result = pipeline.run(&mut ctx).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.stage_name, "Preflight");
}

#[tokio::test]
async fn test_spawn_process_failure_diagnostics() {
    use aurelia::launch::pipeline::{LaunchPipeline, PipelineContext, LaunchErrorKind};
    use aurelia::launch::stages::spawn_process::SpawnProcessStage;
    use aurelia::infra::runners::CommandSpec;

    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(SpawnProcessStage));

    let mut ctx = PipelineContext::new(123);
    let mut spec = CommandSpec::default();
    // Use an absolute path that is guaranteed not to exist for a reliable NotFound error
    spec.program = std::path::Path::new("/bin/nonexistent_utility_123456789").to_path_buf();

    // We need a dummy runner that will actually try to launch
    struct FailingRunner;
    #[async_trait::async_trait]
    impl aurelia::infra::runners::Runner for FailingRunner {
        fn name(&self) -> &str { "FailingRunner" }
        async fn prepare_prefix(&self, _: &aurelia::infra::runners::LaunchContext) -> Result<(), aurelia::launch::pipeline::LaunchError> { Ok(()) }
        async fn build_env(&self, _: &aurelia::infra::runners::LaunchContext) -> Result<std::collections::HashMap<String, String>, aurelia::launch::pipeline::LaunchError> { Ok(std::collections::HashMap::new()) }
        async fn build_command(&self, _: &aurelia::infra::runners::LaunchContext) -> Result<CommandSpec, aurelia::launch::pipeline::LaunchError> { Ok(CommandSpec::default()) }
        fn launch(&self, spec: &CommandSpec) -> Result<std::process::Child, aurelia::launch::pipeline::LaunchError> {
            let res = std::process::Command::new(&spec.program).spawn();
            match res {
                Ok(child) => Ok(child),
                Err(e) => Err(aurelia::launch::pipeline::LaunchError::new(LaunchErrorKind::Process, "spawn failed").with_source(anyhow::anyhow!(e))),
            }
        }
    }

    ctx.runner = Some(Box::new(FailingRunner));
    ctx.command_spec = Some(spec);

    let result = pipeline.run(&mut ctx).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.stage_name, "SpawnProcess");
    assert_eq!(err.inner.kind, LaunchErrorKind::GameData); // NotFound maps to GameData
    assert!(err.inner.message.contains("not found"));
    assert_eq!(err.inner.context.get("io_kind").unwrap(), "NotFound");
}

#[tokio::test]
async fn test_spawn_failure_with_synthetic_lock_shows_hint() {
    use aurelia::launch::pipeline::{LaunchPipeline, PipelineContext, LaunchErrorKind};
    use aurelia::launch::stages::spawn_process::SpawnProcessStage;
    use aurelia::infra::runners::CommandSpec;

    let tmp = tempfile::tempdir().unwrap();
    let lockfile = tmp.path().join(".aurelia_launch.lock");
    std::fs::write(&lockfile, "").unwrap();

    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(SpawnProcessStage));

    let mut ctx = PipelineContext::new(123);
    let mut spec = CommandSpec::default();
    spec.program = std::path::Path::new("/bin/ls").to_path_buf(); // valid exe
    spec.cwd = Some(tmp.path().to_path_buf());

    struct FailingRunner;
    #[async_trait::async_trait]
    impl aurelia::infra::runners::Runner for FailingRunner {
        fn name(&self) -> &str { "FailingRunner" }
        async fn prepare_prefix(&self, _: &aurelia::infra::runners::LaunchContext) -> Result<(), aurelia::launch::pipeline::LaunchError> { Ok(()) }
        async fn build_env(&self, _: &aurelia::infra::runners::LaunchContext) -> Result<std::collections::HashMap<String, String>, aurelia::launch::pipeline::LaunchError> { Ok(std::collections::HashMap::new()) }
        async fn build_command(&self, _: &aurelia::infra::runners::LaunchContext) -> Result<CommandSpec, aurelia::launch::pipeline::LaunchError> { Ok(CommandSpec::default()) }
        fn launch(&self, _: &CommandSpec) -> Result<std::process::Child, aurelia::launch::pipeline::LaunchError> {
            // Force a generic spawn failure
            Err(aurelia::launch::pipeline::LaunchError::new(LaunchErrorKind::Process, "generic spawn failure")
                .with_source(anyhow::anyhow!(std::io::Error::new(std::io::ErrorKind::Other, "something went wrong"))))
        }
    }

    ctx.runner = Some(Box::new(FailingRunner));
    ctx.command_spec = Some(spec);

    let result = pipeline.run(&mut ctx).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.stage_name, "SpawnProcess");
    assert!(err.inner.message.contains("Ensure no other instance is running"));
    assert_eq!(err.inner.context.get("duplicate_instance_detected").unwrap(), "true");
    assert_eq!(err.inner.context.get("duplicate_detection_source").unwrap(), "lockfile");
}

#[tokio::test]
async fn test_launch_artifacts_generation() {
    use aurelia::launch::pipeline::{LaunchPipeline, PipelineContext};
    use aurelia::launch::stages::preflight::PreflightStage;
    use aurelia::infra::runners::CommandSpec;
    use aurelia::infra::logging::LaunchSession;
    use tempfile::tempdir;

    let tmp_logs = tempdir().unwrap();
    let session = LaunchSession::new(tmp_logs.path());

    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(PreflightStage));

    let mut ctx = PipelineContext::new(123);
    let mut spec = CommandSpec::default();
    spec.program = std::path::Path::new("/bin/ls").to_path_buf();
    spec.env.insert("WINEPREFIX".to_string(), "/tmp/fake_pfx".to_string());
    ctx.command_spec = Some(spec);
    ctx.session = Some(session);

    // This should fail preflight because WINEPREFIX doesn't exist
    let _ = pipeline.run(&mut ctx).await;

    let session_dir = ctx.session.as_ref().unwrap().log_dir.clone();

    // Debug: list files in session_dir
    println!("Session dir files: {:?}", std::fs::read_dir(&session_dir).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>());

    // 1. Check preflight_report.json
    assert!(session_dir.join("preflight_report.json").exists());

    // 2. Check effective_env.txt (should be written in write_summary_if_possible on failure)
    assert!(session_dir.join("effective_env.txt").exists());

    // 3. Check command.txt
    assert!(session_dir.join("command.txt").exists());

    // 4. Check dll_resolution.json
    assert!(session_dir.join("dll_resolution.json").exists());

    // Verify content of effective_env.txt is sorted
    let env_content = std::fs::read_to_string(session_dir.join("effective_env.txt")).unwrap();
    assert!(env_content.contains("WINEPREFIX=/tmp/fake_pfx"));
}
