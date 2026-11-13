use super::*;

const MTIME: Option<SystemTime> = Some(SystemTime::UNIX_EPOCH);

fn slot(client: bool, session_mtime: Option<SystemTime>, last_failure: Option<Instant>) -> Slot {
    Slot {
        client: client.then(|| SteamClient::new().unwrap()),
        session_mtime,
        last_failure,
        last_error: None,
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

#[test]
fn header_round_trips_with_and_without_config_dir() {
    let old: Header = serde_json::from_str(r#"{"argv":["aurelia","list"]}"#).unwrap();
    assert_eq!(old.argv, ["aurelia", "list"]);
    assert!(old.config_dir.is_none());
    assert!(old.cwd.is_none());

    let h = Header {
        argv: vec!["aurelia".into()],
        config_dir: Some("/tmp/iso".into()),
        cwd: Some("/home/u/shots".into()),
    };
    let back: Header = serde_json::from_slice(&serde_json::to_vec(&h).unwrap()).unwrap();
    assert_eq!(
        back.config_dir.as_deref(),
        Some(std::path::Path::new("/tmp/iso"))
    );
    assert_eq!(
        back.cwd.as_deref(),
        Some(std::path::Path::new("/home/u/shots"))
    );
}

#[test]
fn config_dir_mismatch_refuses_only_differing_dirs() {
    use std::path::Path;
    let a = Path::new("/home/u/.config/Aurelia");
    let b = Path::new("/tmp/iso");
    assert!(config_dir_mismatch(Some(a), Some(a)).is_none());
    assert!(config_dir_mismatch(None, Some(a)).is_none());
    let msg = config_dir_mismatch(Some(b), Some(a)).unwrap();
    assert!(msg.contains("/tmp/iso") && msg.contains("AURELIA_DAEMON_SOCKET"));
}

fn bare_friend(steam_id: u64, name: Option<&str>) -> Friend {
    Friend {
        steam_id,
        relationship: 3,
        persona_name: name.map(str::to_owned),
        persona_state: None,
        game_app_id: None,
        game_name: None,
    }
}

#[tokio::test]
async fn names_arriving_within_wait_are_awaited() {
    let roster = Arc::new(StdRwLock::new(Roster::new()));
    roster.write().unwrap().insert(1, bare_friend(1, None));
    let writer = Arc::clone(&roster);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        writer
            .write()
            .unwrap()
            .insert(1, bare_friend(1, Some("Alice")));
    });
    let start = Instant::now();
    await_roster_names(&roster, Duration::from_secs(5)).await;
    assert!(start.elapsed() < Duration::from_secs(4));
    let name = roster.read().unwrap()[&1].persona_name.clone();
    assert_eq!(name.as_deref(), Some("Alice"));
}

#[tokio::test]
async fn missing_names_give_up_at_deadline() {
    let roster = StdRwLock::new(Roster::new());
    roster.write().unwrap().insert(1, bare_friend(1, None));
    let start = Instant::now();
    await_roster_names(&roster, Duration::from_millis(300)).await;
    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_millis(300) && elapsed < Duration::from_secs(2));
}

#[tokio::test]
async fn named_or_non_friend_roster_returns_at_once() {
    let roster = StdRwLock::new(Roster::new());
    roster
        .write()
        .unwrap()
        .insert(1, bare_friend(1, Some("Bob")));
    let mut pending = bare_friend(2, None);
    pending.relationship = 2;
    roster.write().unwrap().insert(2, pending);
    let start = Instant::now();
    await_roster_names(&roster, Duration::from_secs(5)).await;
    assert!(start.elapsed() < Duration::from_millis(200));
}
