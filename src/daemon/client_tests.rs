use super::super::{current_build_id, DaemonInfo};
use super::daemon_needs_restart;

fn info(version: &str) -> DaemonInfo {
    DaemonInfo {
        version: version.to_string(),
        pid: 1234,
        build_id: current_build_id(),
    }
}

#[test]
fn same_version_is_reused() {
    assert!(!daemon_needs_restart(Some(&info("0.1.20")), "0.1.20"));
}

#[test]
fn different_version_triggers_restart() {
    assert!(daemon_needs_restart(Some(&info("0.1.19")), "0.1.20"));
    assert!(daemon_needs_restart(Some(&info("0.2.0")), "0.1.20"));
}

/// An old daemon predating the marker leaves it absent; treat "unknown" as a mismatch
/// since such a daemon can't parse newer commands anyway.
#[test]
fn missing_marker_triggers_restart() {
    assert!(daemon_needs_restart(None, "0.1.20"));
}

/// A rebuild at the same crate version changes the binary identity.
#[test]
fn different_build_id_triggers_restart() {
    let mut stale = info("0.1.20");
    stale.build_id = Some("12345-678".to_string());
    assert!(daemon_needs_restart(Some(&stale), "0.1.20"));
}

/// A marker from a build predating the identity field restarts conservatively.
#[test]
fn missing_build_id_triggers_restart() {
    let mut old = info("0.1.20");
    old.build_id = None;
    assert!(daemon_needs_restart(Some(&old), "0.1.20"));
}
