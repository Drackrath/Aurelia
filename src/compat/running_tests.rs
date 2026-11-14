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

fn launch(app_id: u32, pid: u32, ns: Option<u64>) -> RunningGame {
    let mut game = RunningGame::new(app_id, "g".into(), pid, None).with_stand_in(true);
    game.mnt_ns = ns;
    game
}

/// Fake tree: bwrap 10, game 50.
fn fake_parent(pid: u32) -> Option<u32> {
    match pid {
        11 => Some(10),
        10 | 50 | 60 | 70 => Some(1),
        _ => None,
    }
}

fn fake_ns(pid: u32) -> Option<u64> {
    match pid {
        // Host: bwrap itself and others.
        1 | 10 | 60 => Some(100),
        11 | 50 => Some(200),
        70 => Some(300),
        _ => None,
    }
}

fn fake_children(pid: u32) -> Vec<u32> {
    if pid == 10 { vec![11] } else { Vec::new() }
}

#[test]
fn ns_links_parse() {
    assert_eq!(parse_ns_link("mnt:[4026532792]"), Some(4_026_532_792));
    assert_eq!(parse_ns_link("net:[1]"), None);
    assert_eq!(parse_ns_link("mnt:[x]"), None);
}

#[test]
fn reparented_process_in_the_sandbox_is_accepted() {
    let launches = [launch(945_360, 10, None)];
    let m = |peer| {
        match_launch(
            peer,
            &launches,
            fake_parent,
            fake_ns,
            fake_children,
            Some(100),
        )
    };
    // Ancestry still works.
    assert_eq!(m(11), Some(945_360));
    // Reparented to systemd, same namespace.
    assert_eq!(m(50), Some(945_360));
    // Host namespace: refused.
    assert_eq!(m(60), None);
    // Another namespace: refused.
    assert_eq!(m(70), None);
    // Unknown namespace: refused.
    assert_eq!(m(99), None);
}

#[test]
fn recorded_namespace_survives_a_gone_tree() {
    // bwrap exited; record kept ns.
    let launches = [launch(945_360, 999, Some(200))];
    let m = |peer| {
        match_launch(
            peer,
            &launches,
            fake_parent,
            fake_ns,
            fake_children,
            Some(100),
        )
    };
    assert_eq!(m(50), Some(945_360));
    assert_eq!(m(60), None);
}

#[test]
fn namespaces_pick_their_own_launch() {
    let launches = [launch(1, 10, None), launch(2, 999, Some(300))];
    let m = |peer| {
        match_launch(
            peer,
            &launches,
            fake_parent,
            fake_ns,
            fake_children,
            Some(100),
        )
    };
    assert_eq!(m(50), Some(1));
    assert_eq!(m(70), Some(2), "app check runs later");
    // Not stand-in: never matched.
    let mut plain = launch(3, 10, None);
    plain.stand_in = false;
    let only_plain = [plain];
    let m = |peer| {
        match_launch(
            peer,
            &only_plain,
            fake_parent,
            fake_ns,
            fake_children,
            Some(100),
        )
    };
    assert_eq!(m(50), None);
}

#[test]
fn host_namespace_never_matches_even_if_recorded() {
    let launches = [launch(1, 999, Some(100))];
    let m = |peer| {
        match_launch(
            peer,
            &launches,
            fake_parent,
            fake_ns,
            fake_children,
            Some(100),
        )
    };
    assert_eq!(m(60), None);
}

#[test]
fn namespace_round_trips_and_defaults_off() {
    let game = launch(480, 1, Some(42));
    let back: RunningGame = serde_json::from_str(&serde_json::to_string(&game).unwrap()).unwrap();
    assert_eq!(back.mnt_ns, Some(42));
    let old: RunningGame = serde_json::from_str(r#"{"app_id":1,"name":"a","pid":2}"#).unwrap();
    assert_eq!(old.mnt_ns, None);
}

#[cfg(target_os = "linux")]
#[test]
fn real_bwrap_child_matches_its_launch() {
    let Some(bwrap) = crate::launch::steamworks_overlay::find_bwrap() else {
        eprintln!("bwrap not installed; skipped");
        return;
    };
    let mut child = std::process::Command::new(bwrap)
        .args(["--dev-bind", "/", "/", "sleep", "30"])
        .spawn()
        .unwrap();
    let pid = child.id();
    let host = host_mnt_ns();
    assert!(host.is_some());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let inner = loop {
        if let Some(&c) = children_of(pid).first()
            && mnt_ns(c).is_some_and(|ns| Some(ns) != host)
        {
            break c;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("no sandbox namespace (bwrap unusable here); skipped");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let launches = [launch(7, pid, None)];
    let accept = |peer| match_launch(peer, &launches, |_| None, mnt_ns, children_of, host);
    // No ancestry given: namespace decides.
    assert_eq!(accept(inner), Some(7));
    assert_eq!(accept(std::process::id()), None, "host refused");
    let _ = child.kill();
    let _ = child.wait();
}
