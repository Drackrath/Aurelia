use super::*;
use crate::infra::runners::CommandSpec;
use tempfile::tempdir;
use std::fs;

#[tokio::test]
async fn test_preflight_missing_exe() {
    let mut ctx = PipelineContext::new(123);
    let mut spec = CommandSpec::default();
    spec.program = Path::new("/tmp/nonexistent_exe_12345").to_path_buf();
    ctx.command_spec = Some(spec);

    let stage = PreflightStage;
    let res = stage.execute(&mut ctx).await;

    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(err.message.contains("not found"));
    assert!(err.message.contains("[Preflight]"));
}

#[tokio::test]
async fn test_preflight_missing_cwd() {
    let tmp = tempdir().unwrap();
    let exe = tmp.path().join("game.exe");
    fs::write(&exe, "dummy").unwrap();

    let mut ctx = PipelineContext::new(123);
    let mut spec = CommandSpec::default();
    spec.program = exe;
    spec.cwd = Some(tmp.path().join("missing_dir"));
    ctx.command_spec = Some(spec);

    let stage = PreflightStage;
    let res = stage.execute(&mut ctx).await;

    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(err.message.contains("Working directory does not exist"));
}

#[tokio::test]
async fn test_preflight_missing_prefix() {
    let tmp = tempdir().unwrap();
    let exe = tmp.path().join("game.exe");
    fs::write(&exe, "dummy").unwrap();

    let mut ctx = PipelineContext::new(123);
    let mut spec = CommandSpec::default();
    spec.program = exe;
    spec.env.insert("WINEPREFIX".to_string(), tmp.path().join("missing_pfx").to_string_lossy().to_string());
    ctx.command_spec = Some(spec);

    let stage = PreflightStage;
    let res = stage.execute(&mut ctx).await;

    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(err.message.contains("WINEPREFIX does not exist"));
}

#[tokio::test]
async fn test_preflight_is_not_directory() {
    let tmp = tempdir().unwrap();
    let exe = tmp.path().join("game.exe");
    fs::write(&exe, "dummy").unwrap();
    let not_a_dir = tmp.path().join("not_a_dir");
    fs::write(&not_a_dir, "dummy").unwrap();

    let mut ctx = PipelineContext::new(123);
    let mut spec = CommandSpec::default();
    spec.program = exe;
    spec.cwd = Some(not_a_dir);
    ctx.command_spec = Some(spec);

    let stage = PreflightStage;
    let res = stage.execute(&mut ctx).await;

    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(err.message.contains("is not a directory"));
}

#[tokio::test]
async fn test_preflight_umu_run_allowed() {
    // With the umu plugin active, `spec.program` is the absolute plugin-resolved
    // `umu-run` binary; an existing absolute path must pass preflight normally.
    let tmp = tempdir().unwrap();
    let umu_run = tmp.path().join("umu-run");
    fs::write(&umu_run, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&umu_run).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&umu_run, perms).unwrap();
    }

    let mut ctx = PipelineContext::new(123);
    let mut spec = CommandSpec::default();
    spec.program = umu_run;
    ctx.command_spec = Some(spec);

    let stage = PreflightStage;
    let res = stage.execute(&mut ctx).await;

    assert!(res.is_ok(), "resolved umu-run should pass preflight: {:?}", res.err());
}

#[tokio::test]
async fn test_preflight_bogus_relative_runner_fails() {
    // A bogus relative, non-existent runner path must fail preflight.
    let mut ctx = PipelineContext::new(123);
    let mut spec = CommandSpec::default();
    spec.program = Path::new("umu-run").to_path_buf();
    ctx.command_spec = Some(spec);

    let stage = PreflightStage;
    let res = stage.execute(&mut ctx).await;

    assert!(res.is_err(), "non-existent runner should fail preflight");
}

#[tokio::test]
#[cfg(unix)]
async fn test_preflight_native_game_self_heals_exec_bit() {
    use std::os::unix::fs::PermissionsExt;
    use crate::core::models::LibraryGame;
    use crate::steam_client::{LaunchInfo, LaunchTarget};

    let tmp = tempdir().unwrap();
    let exe = tmp.path().join("Game.AppImage");
    fs::write(&exe, "dummy").unwrap();
    let mut perms = fs::metadata(&exe).unwrap().permissions();
    perms.set_mode(0o644); // Not executable
    fs::set_permissions(&exe, perms).unwrap();

    let mut ctx = PipelineContext::new(123);
    ctx.app = Some(LibraryGame {
        app_id: 123,
        name: "Test Game".to_string(),
        playtime_forever_minutes: None,
        is_installed: true,
        install_path: Some(tmp.path().to_string_lossy().to_string()),
        local_manifest_ids: Default::default(),
        update_available: false,
        update_queued: false,
        active_branch: "public".to_string(),
        is_owned: true,
        is_family_shared: false,
        online_required: None,
        platform: None,
        from_windows_steam: false,
    });
    ctx.launch_info = Some(LaunchInfo {
        app_id: 123,
        id: "0".to_string(),
        description: "Test".to_string(),
        executable: "Game.AppImage".to_string(),
        arguments: String::new(),
        workingdir: None,
        target: LaunchTarget::NativeLinux,
    });
    let mut spec = CommandSpec::default();
    spec.program = exe.clone();
    ctx.command_spec = Some(spec);

    let stage = PreflightStage;
    let res = stage.execute(&mut ctx).await;

    assert!(res.is_ok(), "native game should self-heal: {:?}", res.err());
    let mode = fs::metadata(&exe).unwrap().permissions().mode();
    assert!(mode & 0o111 != 0, "exec bit should be set, mode {mode:o}");
}

#[tokio::test]
async fn test_preflight_native_ignores_path_like_argument() {
    use crate::core::models::LibraryGame;
    use crate::steam_client::{LaunchInfo, LaunchTarget};

    let tmp = tempdir().unwrap();
    let exe = tmp.path().join("game.bin");
    fs::write(&exe, "dummy").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&exe).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&exe, perms).unwrap();
    }

    let mut ctx = PipelineContext::new(123);
    ctx.app = Some(LibraryGame {
        app_id: 123,
        name: "Test Game".to_string(),
        playtime_forever_minutes: None,
        is_installed: true,
        install_path: Some(tmp.path().to_string_lossy().to_string()),
        local_manifest_ids: Default::default(),
        update_available: false,
        update_queued: false,
        active_branch: "public".to_string(),
        is_owned: true,
        is_family_shared: false,
        online_required: None,
        platform: None,
        from_windows_steam: false,
    });
    ctx.launch_info = Some(LaunchInfo {
        app_id: 123,
        id: "0".to_string(),
        description: "Test".to_string(),
        executable: "game.bin".to_string(),
        arguments: "data/missing.ini".to_string(),
        workingdir: None,
        target: LaunchTarget::NativeLinux,
    });
    let mut spec = CommandSpec::default();
    spec.program = exe.clone();
    spec.args = vec!["data/missing.ini".to_string()];
    ctx.command_spec = Some(spec);

    let stage = PreflightStage;
    let res = stage.execute(&mut ctx).await;

    assert!(res.is_ok(), "native args must not be treated as exe: {:?}", res.err());
    assert_eq!(ctx.resolved_executable_path, Some(exe));
    assert!(ctx.executable_exists);
}

#[tokio::test]
#[cfg(unix)]
async fn test_preflight_not_executable() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempdir().unwrap();
    let exe = tmp.path().join("game.exe");
    fs::write(&exe, "dummy").unwrap();
    let mut perms = fs::metadata(&exe).unwrap().permissions();
    perms.set_mode(0o644); // Not executable
    fs::set_permissions(&exe, perms).unwrap();

    let mut ctx = PipelineContext::new(123);
    let mut spec = CommandSpec::default();
    spec.program = exe;
    ctx.command_spec = Some(spec);

    let stage = PreflightStage;
    let res = stage.execute(&mut ctx).await;

    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(err.message.contains("is not executable"));
}
