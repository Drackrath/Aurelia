use super::*;

#[test]
fn new_record_owns_its_live_pid() {
    let game = RunningGame::new(70, "Half-Life".into(), std::process::id(), None);
    assert!(game.pid_start_time.is_some());
    assert!(game.owns_pid());
}

#[test]
fn recycled_pid_is_not_owned() {
    // Same PID, different start time: what a reused PID looks like after a
    // crash or reboot.
    let mut game = RunningGame::new(70, "Half-Life".into(), std::process::id(), None);
    game.pid_start_time = game.pid_start_time.map(|t| t.wrapping_sub(3600));
    assert!(!game.owns_pid());
}

#[test]
fn record_without_start_time_never_owns_pid() {
    // Records written before start times were tracked deserialize with `None`
    // and must not authorise signalling whatever holds the PID now.
    let legacy = r#"{ "app_id": 70, "name": "Half-Life", "pid": 1 }"#;
    let mut game: RunningGame = serde_json::from_str(legacy).unwrap();
    assert_eq!(game.pid_start_time, None);
    assert_eq!(game.wineprefix, None);
    game.pid = std::process::id();
    assert!(!game.owns_pid());
}

#[test]
fn set_pid_refreshes_start_time() {
    let mut game = RunningGame::new(70, "Half-Life".into(), u32::MAX, None);
    assert_eq!(game.pid_start_time, None);
    game.set_pid(std::process::id());
    assert!(game.owns_pid());
}

#[test]
fn start_time_of_missing_process_is_none() {
    assert_eq!(process_start_time(u32::MAX), None);
}
