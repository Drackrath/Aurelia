use super::super::steam_running_among;

const ME: u32 = 501;

#[test]
fn live_steam_osx_is_running() {
    let quit = [("ipcserver", Some(ME)), ("Steam Helper", Some(ME))];
    assert!(!steam_running_among(quit, ME));
    let running = [("ipcserver", Some(ME)), ("steam_osx", Some(ME))];
    assert!(steam_running_among(running, ME));
}

#[test]
fn other_users_steam_is_not_running() {
    let others = [("steam_osx", Some(ME + 1)), ("steam_osx", None)];
    assert!(!steam_running_among(others, ME));
}
