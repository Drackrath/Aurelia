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

#[test]
fn descent_follows_parents() {
    // 10 -> 5 -> 1.
    let parent = |pid: u32| match pid {
        10 => Some(5),
        5 => Some(1),
        _ => None,
    };
    assert!(descends_from(10, 10, parent));
    assert!(descends_from(10, 5, parent));
    assert!(descends_from(10, 1, parent));
    assert!(!descends_from(5, 10, parent));
    assert!(!descends_from(77, 5, parent));
    // Self-parent loops stop.
    assert!(!descends_from(3, 9, Some));
}

#[cfg(target_os = "linux")]
#[test]
fn this_process_descends_from_its_parent() {
    let me = std::process::id();
    let parent = std::os::unix::process::parent_id();
    assert_eq!(parent_pid(me), Some(parent));
    assert!(descends_from(me, parent, parent_pid));
    assert!(!descends_from(parent, me, parent_pid));
}

#[test]
fn stand_in_flag_round_trips_and_defaults_off() {
    let game = RunningGame::new(480, "x".into(), 1, None).with_stand_in(true);
    let json = serde_json::to_string(&game).unwrap();
    let back: RunningGame = serde_json::from_str(&json).unwrap();
    assert!(back.stand_in);
    let old: RunningGame = serde_json::from_str(r#"{"app_id":1,"name":"a","pid":2}"#).unwrap();
    assert!(!old.stand_in);
}
