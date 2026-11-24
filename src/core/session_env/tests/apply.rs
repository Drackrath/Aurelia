use crate::core::session_env::*;

fn env_of(cmd: &std::process::Command, key: &str) -> Option<Option<String>> {
    cmd.get_envs()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
}

#[test]
fn from_pairs_keeps_only_session_vars() {
    let env = SessionEnv::from_pairs([("DISPLAY", ":6"), ("HOME", "/h"), ("WAYLAND_DISPLAY", "")]);
    assert_eq!(env.get("DISPLAY"), Some(":6"));
    assert_eq!(env.get("HOME"), None);
    assert_eq!(env.get("WAYLAND_DISPLAY"), None);
}

#[test]
fn apply_sets_present_and_removes_absent() {
    let env = SessionEnv::from_pairs([("DISPLAY", ":6")]);
    let mut cmd = std::process::Command::new("true");
    env.apply(&mut cmd);
    assert_eq!(env_of(&cmd, "DISPLAY"), Some(Some(":6".into())));
    // Removal is recorded as None.
    assert_eq!(env_of(&cmd, "WAYLAND_DISPLAY"), Some(None));
    assert_eq!(env_of(&cmd, "XAUTHORITY"), Some(None));
}

#[test]
fn x11_reachability_checks_local_socket() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("X6"), b"").unwrap();
    assert!(x11_display_reachable(":6", dir.path()));
    assert!(x11_display_reachable(":6.0", dir.path()));
    assert!(!x11_display_reachable(":7", dir.path()));
    assert!(!x11_display_reachable(":", dir.path()));
    assert!(x11_display_reachable("remote:0", dir.path()));
}

#[test]
fn wayland_only_without_display() {
    let env = SessionEnv::from_pairs([("WAYLAND_DISPLAY", "wayland-1")]);
    assert!(env.is_wayland_only());
    assert!(!SessionEnv::default().is_wayland_only());
}
