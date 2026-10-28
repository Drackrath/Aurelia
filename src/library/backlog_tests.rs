use super::*;

fn entry(app_id: u32, state: BacklogState) -> BacklogEntry {
    BacklogEntry {
        app_id,
        name: format!("Game {app_id}"),
        state,
        platform: "windows".into(),
        library: "/lib".into(),
        install_dir: None,
        is_dlc: false,
        restart_steam: false,
        downloaded_bytes: 10,
        total_bytes: 100,
        reason: None,
        error_type: None,
        auto_attempts: 0,
        added_at: 1,
        updated_at: 1,
    }
}

#[test]
fn missing_file_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load_from(&dir.path().join("none.json")).unwrap().is_empty());
}

#[test]
fn round_trips_and_replaces_by_app_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("install_backlog.json");
    update_at(&path, |e| upsert_in(e, entry(1, BacklogState::Interrupted))).unwrap();
    update_at(&path, |e| upsert_in(e, entry(2, BacklogState::Paused))).unwrap();
    let mut again = entry(1, BacklogState::Aborted);
    again.added_at = 99;
    update_at(&path, |e| upsert_in(e, again)).unwrap();
    let all = load_from(&path).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].state, BacklogState::Aborted);
    // The first add time survives.
    assert_eq!(all[0].added_at, 1);
    assert!(!path.with_extension("json.tmp").exists());
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(
        raw.contains("\"state\": \"aborted\"") && raw.contains("\"version\": 1"),
        "{raw}"
    );
}

#[test]
fn corrupt_file_is_an_error_not_wiped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("install_backlog.json");
    std::fs::write(&path, "{not json").unwrap();
    assert!(update_at(&path, |e| e.clear()).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
}

#[test]
fn auto_resume_takes_only_interrupted_under_the_cap() {
    let mut capped = entry(4, BacklogState::Interrupted);
    capped.auto_attempts = MAX_AUTO_ATTEMPTS;
    let entries = [
        entry(1, BacklogState::Interrupted),
        entry(2, BacklogState::Paused),
        entry(3, BacklogState::Aborted),
        capped,
        entry(5, BacklogState::Queued),
    ];
    assert_eq!(auto_resumable(&entries), vec![1]);
}

#[test]
fn deletable_dir_guards() {
    let lib = tempfile::tempdir().unwrap();
    let common = lib.path().join("steamapps").join("common");
    std::fs::create_dir_all(&common).unwrap();
    let mut e = entry(7, BacklogState::Aborted);
    e.library = lib.path().to_string_lossy().into_owned();
    e.install_dir = Some(common.join("Game"));
    assert_eq!(deletable_dir(&e), Some(common.join("Game")));

    let mut dlc = e.clone();
    dlc.is_dlc = true;
    assert_eq!(deletable_dir(&dlc), None);

    let mut outside = e.clone();
    outside.install_dir = Some(lib.path().join("elsewhere"));
    assert_eq!(deletable_dir(&outside), None);

    let mut dotdot = e.clone();
    dotdot.install_dir = Some(common.join(".."));
    assert_eq!(deletable_dir(&dotdot), None);

    std::fs::write(lib.path().join("steamapps").join("appmanifest_7.acf"), "").unwrap();
    assert_eq!(deletable_dir(&e), None);
}

#[test]
fn early_failures_still_use_up_auto_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("install_backlog.json");
    update_at(&path, |e| upsert_in(e, entry(1, BacklogState::Interrupted))).unwrap();
    for expected in 1..=MAX_AUTO_ATTEMPTS {
        let bumped = update_at(&path, |e| bump_in(e, 1)).unwrap();
        assert_eq!(bumped, Some(expected));
    }
    assert!(auto_resumable(&load_from(&path).unwrap()).is_empty());
    assert_eq!(update_at(&path, |e| bump_in(e, 2)).unwrap(), None);
}
