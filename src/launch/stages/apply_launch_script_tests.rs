use super::*;
use crate::infra::runners::CommandSpec;
use std::path::Path;
use tempfile::tempdir;

#[tokio::test]
async fn wraps_spec_with_override_script() {
    let tmp = tempdir().unwrap();
    let script = tmp.path().join("wrap.sh");
    std::fs::write(&script, "exec \"$@\"\n").unwrap();

    let mut ctx = PipelineContext::new(555);
    let mut spec = CommandSpec::default();
    spec.program = PathBuf::from("/usr/bin/proton");
    spec.args = vec!["run".to_string(), "game.exe".to_string()];
    ctx.command_spec = Some(spec);
    ctx.launch_script_override = Some(script.clone());

    let stage = ApplyLaunchScriptStage;
    stage.execute(&mut ctx).await.unwrap();

    let spec = ctx.command_spec.unwrap();
    assert_eq!(spec.program, script);
    assert_eq!(spec.args, vec!["/usr/bin/proton", "run", "game.exe"]);
    assert_eq!(spec.env.get("AURELIA_APP_ID").map(String::as_str), Some("555"));
    assert_eq!(
        spec.env.get("AURELIA_LAUNCH_PROGRAM").map(String::as_str),
        Some("/usr/bin/proton")
    );
    assert_eq!(
        spec.env.get("AURELIA_LAUNCH_ARGS").map(String::as_str),
        Some("run game.exe")
    );
}

#[tokio::test]
async fn missing_explicit_script_errors() {
    let mut ctx = PipelineContext::new(555);
    let mut spec = CommandSpec::default();
    spec.program = PathBuf::from("/usr/bin/proton");
    ctx.command_spec = Some(spec);
    ctx.launch_script_override = Some(PathBuf::from("/no/such/script.sh"));

    let stage = ApplyLaunchScriptStage;
    let res = stage.execute(&mut ctx).await;
    assert!(res.is_err());
    assert_eq!(res.unwrap_err().kind, LaunchErrorKind::Validation);
}

#[tokio::test]
async fn no_script_is_noop() {
    let _guard = crate::launch::launch_script::SCRIPT_DIR_ENV_LOCK.lock().unwrap();
    let tmp = tempdir().unwrap();
    unsafe { std::env::set_var("AURELIA_SCRIPT_DIR", tmp.path()) };
    let mut ctx = PipelineContext::new(555);
    let mut spec = CommandSpec::default();
    spec.program = PathBuf::from("/usr/bin/proton");
    spec.args = vec!["run".to_string()];
    ctx.command_spec = Some(spec);

    let stage = ApplyLaunchScriptStage;
    stage.execute(&mut ctx).await.unwrap();

    let spec = ctx.command_spec.unwrap();
    assert_eq!(spec.program, Path::new("/usr/bin/proton"));
    assert_eq!(spec.args, vec!["run"]);
    assert!(!spec.env.contains_key("AURELIA_APP_ID"));
    unsafe { std::env::remove_var("AURELIA_SCRIPT_DIR") };
}

#[tokio::test]
async fn disabled_skips_wrapping() {
    let tmp = tempdir().unwrap();
    let script = tmp.path().join("wrap.sh");
    std::fs::write(&script, "exec \"$@\"\n").unwrap();

    let mut ctx = PipelineContext::new(555);
    let mut spec = CommandSpec::default();
    spec.program = PathBuf::from("/usr/bin/proton");
    ctx.command_spec = Some(spec);
    ctx.launch_script_override = Some(script);
    ctx.disable_launch_script = true;

    let stage = ApplyLaunchScriptStage;
    stage.execute(&mut ctx).await.unwrap();
    assert_eq!(ctx.command_spec.unwrap().program, Path::new("/usr/bin/proton"));
}
