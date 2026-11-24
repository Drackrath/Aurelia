use crate::core::session_env::SessionEnv;
use crate::core::steam_host::check_x11_for_host_steam;

#[test]
fn wayland_only_session_is_refused() {
    let session = SessionEnv::from_pairs([("WAYLAND_DISPLAY", "wayland-4")]);
    let err = check_x11_for_host_steam(&session).unwrap_err();
    assert!(err.to_string().contains("needs an X server"));
}

#[test]
fn remote_x_display_passes() {
    let session = SessionEnv::from_pairs([("DISPLAY", "otherhost:0")]);
    assert!(check_x11_for_host_steam(&session).is_ok());
}
