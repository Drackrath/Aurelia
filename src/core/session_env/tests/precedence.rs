use crate::core::session_env::*;

fn base() -> SessionEnv {
    SessionEnv::from_pairs([
        ("DISPLAY", ":0"),
        ("WAYLAND_DISPLAY", "wayland-1"),
        ("XAUTHORITY", "/run/user/1000/xauth_kde"),
        ("XDG_RUNTIME_DIR", "/run/user/1000"),
    ])
}

fn no_tty(_: &str) -> anyhow::Result<SessionEnv> {
    panic!("tty must not be resolved")
}

#[test]
fn empty_config_keeps_base() {
    let env = resolve_target(base(), &DisplayTarget::default(), None, no_tty).unwrap();
    assert_eq!(env, base());
}

#[test]
fn per_game_beats_global_beats_base() {
    let global = DisplayTarget {
        display: Some(":2".into()),
        wayland_display: Some("wayland-2".into()),
        ..Default::default()
    };
    let game = DisplayTarget {
        display: Some(":6".into()),
        ..Default::default()
    };
    let env = resolve_target(base(), &global, Some(&game), no_tty).unwrap();
    assert_eq!(env.get("DISPLAY"), Some(":6"));
    assert_eq!(env.get("WAYLAND_DISPLAY"), Some("wayland-2"));
    assert_eq!(env.get("XAUTHORITY"), Some("/run/user/1000/xauth_kde"));
}

#[test]
fn tty_replaces_display_vars_then_explicit_wins() {
    let global = DisplayTarget {
        tty: Some("tty3".into()),
        xauthority: Some("/x".into()),
        ..Default::default()
    };
    let env = resolve_target(base(), &global, None, |tty| {
        assert_eq!(tty, "tty3");
        Ok(SessionEnv::from_pairs([
            ("DISPLAY", ":6"),
            ("WAYLAND_DISPLAY", "wayland-4"),
        ]))
    })
    .unwrap();
    assert_eq!(env.get("DISPLAY"), Some(":6"));
    assert_eq!(env.get("WAYLAND_DISPLAY"), Some("wayland-4"));
    // Explicit field beats tty.
    assert_eq!(env.get("XAUTHORITY"), Some("/x"));
    // Non-display base vars survive.
    assert_eq!(env.get("XDG_RUNTIME_DIR"), Some("/run/user/1000"));
}

#[test]
fn tty_drops_base_xauthority() {
    let global = DisplayTarget {
        tty: Some("3".into()),
        ..Default::default()
    };
    let env = resolve_target(base(), &global, None, |_| {
        Ok(SessionEnv::from_pairs([("WAYLAND_DISPLAY", "wayland-4")]))
    })
    .unwrap();
    assert_eq!(env.get("DISPLAY"), None);
    assert_eq!(env.get("XAUTHORITY"), None);
}

#[test]
fn tty_error_propagates() {
    let global = DisplayTarget {
        tty: Some("tty9".into()),
        ..Default::default()
    };
    let err = resolve_target(base(), &global, None, |_| anyhow::bail!("no session")).unwrap_err();
    assert!(err.to_string().contains("no session"));
}
