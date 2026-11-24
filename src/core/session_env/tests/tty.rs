use std::collections::HashMap;

use crate::core::session_env::*;

const SHOW: &str = "Id=2\nUser=1000\nVTNr=1\nTTY=tty1\n\nId=3\nUser=1000\nVTNr=2\nTTY=tty2\n\nId=9\nUser=1001\nVTNr=3\nTTY=tty3\n";

#[test]
fn session_id_by_tty_or_vt() {
    assert_eq!(session_id_for_tty(SHOW, "tty2", 1000).as_deref(), Some("3"));
    assert_eq!(session_id_for_tty(SHOW, "1", 1000).as_deref(), Some("2"));
    // Other users' sessions are ignored.
    assert_eq!(session_id_for_tty(SHOW, "tty3", 1000), None);
}

fn environ(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn vote_picks_majority_per_var() {
    let wayfire = [
        ("XDG_SESSION_ID", "3"),
        ("DISPLAY", ":2"),
        ("WAYLAND_DISPLAY", "wayland-2"),
    ];
    let mut procs = vec![environ(&wayfire); 3];
    // Stray inherited KDE cookie.
    let mut stray = environ(&wayfire);
    stray.insert("XAUTHORITY".into(), "/run/user/1000/xauth_kde".into());
    procs.push(stray);
    // Container child with in-container path.
    let mut pv = environ(&wayfire);
    pv.insert(
        "XAUTHORITY".into(),
        "/run/pressure-vessel/Xauthority".into(),
    );
    procs.extend(std::iter::repeat_n(pv, 5));
    // Another session entirely.
    procs.push(environ(&[("XDG_SESSION_ID", "2"), ("DISPLAY", ":0")]));

    let env = vote_session_env("3", &procs);
    assert_eq!(env.get("DISPLAY"), Some(":2"));
    assert_eq!(env.get("WAYLAND_DISPLAY"), Some("wayland-2"));
    assert_eq!(env.get("XAUTHORITY"), None);
}

#[test]
fn vote_ignores_processes_without_display() {
    let procs = vec![environ(&[("XDG_SESSION_ID", "3")])];
    assert_eq!(vote_session_env("3", &procs), SessionEnv::default());
}

#[test]
fn parses_nul_separated_environ() {
    let env = parse_environ(b"A=1\0B=x=y\0junk\0");
    assert_eq!(env.get("A").map(String::as_str), Some("1"));
    assert_eq!(env.get("B").map(String::as_str), Some("x=y"));
    assert_eq!(env.len(), 2);
}
