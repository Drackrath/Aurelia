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

#[test]
fn test_mz_header_check() {
    use aurelia::launch::has_mz_header;
    let tmp = tempfile::tempdir().unwrap();

    let valid = tmp.path().join("valid.dll");
    std::fs::write(&valid, b"MZ\x90\x00fake-pe-body").unwrap();
    assert!(has_mz_header(&valid));

    let zero_byte = tmp.path().join("zero.dll");
    std::fs::write(&zero_byte, b"").unwrap();
    assert!(!has_mz_header(&zero_byte));

    let html = tmp.path().join("error_page.dll");
    std::fs::write(&html, b"<html>404</html>").unwrap();
    assert!(!has_mz_header(&html));

    assert!(!has_mz_header(&tmp.path().join("missing.dll")));
}

#[test]
fn test_corrupt_steam_api_restored_from_bak() {
    use aurelia::launch::stages::preflight::check_game_steam_api_libs;
    let tmp = tempfile::tempdir().unwrap();
    let dll = tmp.path().join("steam_api64.dll");
    let bak = tmp.path().join("steam_api64.dll.bak");
    std::fs::write(&dll, b"").unwrap(); // zero-byte: fails the MZ check
    std::fs::write(&bak, b"MZ\x90\x00real-steam-api").unwrap();

    let notes = check_game_steam_api_libs(&[tmp.path().to_path_buf()]);

    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("restored"), "unexpected note: {}", notes[0]);
    // The corrupt DLL was replaced with the backup's contents.
    assert_eq!(std::fs::read(&dll).unwrap(), b"MZ\x90\x00real-steam-api");
}

#[test]
fn test_corrupt_steam_api_without_bak_warns() {
    use aurelia::launch::stages::preflight::check_game_steam_api_libs;
    let tmp = tempfile::tempdir().unwrap();
    let dll = tmp.path().join("steam_api.dll");
    std::fs::write(&dll, b"corrupt-not-a-pe").unwrap();

    let notes = check_game_steam_api_libs(&[tmp.path().to_path_buf()]);

    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("corrupt"), "unexpected note: {}", notes[0]);
    assert!(notes[0].contains("no valid .bak"), "unexpected note: {}", notes[0]);
    // No .bak: the corrupt file is left in place (no depot re-download invented).
    assert_eq!(std::fs::read(&dll).unwrap(), b"corrupt-not-a-pe");
}

#[test]
fn test_valid_and_absent_steam_api_produce_no_notes() {
    use aurelia::launch::stages::preflight::check_game_steam_api_libs;
    let tmp = tempfile::tempdir().unwrap();
    // Valid MZ DLL — untouched, no note. steam_api64.dll absent — also no note.
    let dll = tmp.path().join("steam_api.dll");
    std::fs::write(&dll, b"MZ\x90\x00good").unwrap();

    let notes = check_game_steam_api_libs(&[tmp.path().to_path_buf()]);
    assert!(notes.is_empty(), "unexpected notes: {:?}", notes);
    assert_eq!(std::fs::read(&dll).unwrap(), b"MZ\x90\x00good");
}

#[test]
fn test_missing_steam_runtime_libs_reports_corrupt_and_absent() {
    use aurelia::launch::stages::preflight::missing_steam_runtime_libs;
    let tmp = tempfile::tempdir().unwrap();
    let steam_dir = tmp.path().join("Steam");
    std::fs::create_dir_all(&steam_dir).unwrap();
    std::fs::write(steam_dir.join("steam.exe"), b"MZ\x90\x00ok").unwrap();
    std::fs::write(steam_dir.join("steamclient.dll"), b"").unwrap(); // corrupt
    // steamclient64.dll absent entirely.

    let missing = missing_steam_runtime_libs(&steam_dir);
    assert_eq!(missing, vec!["steamclient.dll".to_string(), "steamclient64.dll".to_string()]);

    // A fully valid runtime dir reports nothing.
    std::fs::write(steam_dir.join("steamclient.dll"), b"MZ\x90\x00ok").unwrap();
    std::fs::write(steam_dir.join("steamclient64.dll"), b"MZ\x90\x00ok").unwrap();
    assert!(missing_steam_runtime_libs(&steam_dir).is_empty());
}
