use super::super::*;
use crate::launch::context::PipelineContext;
use crate::launch::runners::CommandSpec;

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
