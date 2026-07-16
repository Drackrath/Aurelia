use super::super::DaemonInfo;
use super::daemon_needs_restart;

fn info(version: &str) -> DaemonInfo {
    DaemonInfo {
        version: version.to_string(),
        pid: 1234,
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
