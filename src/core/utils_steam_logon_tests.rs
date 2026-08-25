use super::connection_log_logged_on;

// Log lines shaped like Steam's real connection_log.txt.
const LOGGED_ON_TAIL: &str = "\
[2026-08-25 16:06:55] [Logged On, 4, 7] [U:1:96573820] RecvMsgClientLogOnResponse() : processing complete
[2026-08-25 16:06:55] [Logged On, 4, 7] [U:1:96573820] Connection established
";

const LOGGED_OFF_TAIL: &str = "\
[2026-08-25 16:10:27] [Logged On, 4, 7] [U:1:96573820] LogOff()
[2026-08-25 16:10:28] [Logged Off, 0, 0] [U:1:96573820] Sending SteamServersDisconnected_t
[2026-08-25 16:10:29] [Logged Off, 0, 0] [U:1:96573820] Log session ended
";

#[test]
fn last_state_logged_on_wins() {
    assert_eq!(connection_log_logged_on(LOGGED_ON_TAIL), Some(true));
}

#[test]
fn last_state_logged_off_wins() {
    // Ends logged off even though it was on earlier.
    assert_eq!(connection_log_logged_on(LOGGED_OFF_TAIL), Some(false));
}

#[test]
fn no_markers_is_none() {
    let text = "[2026-08-25 16:00:00] Log session started (verbosity 0)\n";
    assert_eq!(connection_log_logged_on(text), None);
}

#[test]
fn empty_is_none() {
    assert_eq!(connection_log_logged_on(""), None);
}
