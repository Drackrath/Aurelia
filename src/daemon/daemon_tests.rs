use super::*;

const MTIME: Option<SystemTime> = Some(SystemTime::UNIX_EPOCH);

fn slot(client: bool, session_mtime: Option<SystemTime>, last_failure: Option<Instant>) -> Slot {
    Slot {
        client: client.then(|| SteamClient::new().unwrap()),
        session_mtime,
        last_failure,
    }
}

#[test]
fn never_attempted_triggers_a_restore() {
    // Fresh daemon: no client, no recorded failure, mtime differs from None.
    assert!(!slot(false, None, None).is_current(MTIME));
}

#[test]
fn live_session_is_a_no_op() {
    assert!(slot(true, MTIME, None).is_current(MTIME));
}

#[test]
fn token_change_forces_restore_even_with_live_client() {
    let other = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1));
    assert!(!slot(true, MTIME, None).is_current(other));
}

#[test]
fn recent_failure_is_held_off_until_the_backoff_elapses() {
    assert!(slot(false, MTIME, Some(Instant::now())).is_current(MTIME));
}

#[test]
fn stale_failure_self_heals_after_the_backoff() {
    // A failure older than the backoff window must allow another restore attempt —
    // this is the regression guard for the daemon wedging until `login --reconnect`.
    let past = Instant::now()
        .checked_sub(RESTORE_RETRY_BACKOFF + Duration::from_secs(1))
        .expect("instant underflow");
    assert!(!slot(false, MTIME, Some(past)).is_current(MTIME));
}
