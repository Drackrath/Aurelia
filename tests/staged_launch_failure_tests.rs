use aurelia::launch::pipeline::{LaunchPipeline, PipelineContext, LaunchErrorKind};
use aurelia::launch::stages::resolve_game::ResolveGameStage;
use aurelia::launch::stages::resolve_profile::ResolveProfileStage;
use aurelia::launch::stages::spawn_process::SpawnProcessStage;
use aurelia::models::LibraryGame;
use std::collections::HashMap;

#[tokio::test]
async fn test_stage_resolution_failure() {
    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(ResolveGameStage));

    let mut ctx = PipelineContext::new(123);
    // ctx.app is None, so ResolveGameStage should fail

    let result = pipeline.run(&mut ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.stage_name, "ResolveGame");
    assert_eq!(err.inner.kind, LaunchErrorKind::Validation);
    assert!(err.inner.message.contains("App context missing"));
}

#[tokio::test]
async fn test_stage_validation_failure_launch_info() {
    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(ResolveProfileStage));

    let mut ctx = PipelineContext::new(123);
    ctx.app = Some(LibraryGame {
        app_id: 123,
        name: "Test".to_string(),
        install_path: None,
        is_installed: false,
        playtime_forever_minutes: None,
        active_branch: "public".to_string(),
        update_available: false,
        update_queued: false,
        local_manifest_ids: HashMap::new(),
        is_owned: true,
        is_family_shared: false,
        online_required: None,
    });
    // ctx.launch_info is None, so ResolveProfileStage should fail

    let result = pipeline.run(&mut ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.stage_name, "ResolveProfile");
    assert_eq!(err.inner.kind, LaunchErrorKind::Validation);
}

#[tokio::test]
async fn test_stage_execution_failure_adhoc() {
    let mut pipeline = LaunchPipeline::new();
    pipeline.add_stage(Box::new(SpawnProcessStage));

    let mut ctx = PipelineContext::new(123);
    ctx.app = Some(LibraryGame {
        app_id: 123,
        name: "Test".to_string(),
        install_path: Some("/tmp/invalid_path_likely_fails_spawn".to_string()),
        is_installed: true,
        playtime_forever_minutes: None,
        active_branch: "public".to_string(),
        update_available: false,
        update_queued: false,
        local_manifest_ids: HashMap::new(),
        is_owned: true,
        is_family_shared: false,
        online_required: None,
    });
    ctx.launch_info = Some(aurelia::steam_client::LaunchInfo {
        app_id: 123,
        id: "0".to_string(),
        description: "Test".to_string(),
        executable: "not_found.sh".to_string(),
        arguments: "".to_string(),
        workingdir: None,
        target: aurelia::steam_client::LaunchTarget::NativeLinux,
    });
    ctx.launcher_config = Some(aurelia::config::LauncherConfig::default());

    let result = pipeline.run(&mut ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.stage_name, "SpawnProcess");
    // It should fail either during client init or process spawn
    // In our mocked/limited environment, probably process spawn if it gets there.
    // Update: 'NotFound' IO error now maps to LaunchErrorKind::GameData
    assert!(matches!(err.inner.kind, LaunchErrorKind::Process | LaunchErrorKind::Environment | LaunchErrorKind::GameData));
}
