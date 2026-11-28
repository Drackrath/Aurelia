use super::super::{ExitSyncState, LogMark};

const APP: u32 = 2379780;

// Steam's real lines from a run whose exit upload raced ours.
const GAMEPROCESS: &str = "\
[2026-11-27 03:22:23] AppID 2379780 adding PID 15921 as a tracked process
[2026-11-27 03:22:23] SSGL: InternalUpdateClientGame indicates change to games list
[2026-11-27 03:22:24] SSGL: change [2379780] LCT 9923->0
[2026-11-27 03:23:10] AppID 2379780 no longer tracking PID 15921, exit code -1
[2026-11-27 03:23:10] Remove 2379780 from running list
";

const CLOUD: &str = "\
[2026-11-27 03:23:10] [AppID 2379780] Starting sync (up,AC Exit,)
[2026-11-27 03:23:10] [AppID 2379780] Running AutoCloud on exit. Looking for new and updated files
[2026-11-27 03:23:10] [AppID 2379780]     No launch record found. Skipping looking for files added/removed while the game was running
[2026-11-27 03:23:11] [AppID 2379780] Need to upload file Balatro/settings.jkr
[2026-11-27 03:23:11] [AppID 2379780] Failed to begin app upload batch - Too many pending
[2026-11-27 03:23:11] [AppID 2379780] Upload complete, result Too many pending
";

fn head(text: &str, lines: usize) -> String {
    text.lines().take(lines).map(|l| format!("{l}\n")).collect()
}

#[test]
fn waits_until_steam_ends_its_upload() {
    assert_eq!(
        ExitSyncState::from_logs(GAMEPROCESS, &head(CLOUD, 4), APP),
        ExitSyncState::Uploading
    );
    assert_eq!(
        ExitSyncState::from_logs(GAMEPROCESS, CLOUD, APP),
        ExitSyncState::Done
    );
}

#[test]
fn run_steam_never_tracked_waits_for_nothing() {
    // Another app whose id starts with this one's digits.
    let gameprocess =
        "[2026-11-27 03:22:23] AppID 23797801 adding PID 15921 as a tracked process\n";
    assert_eq!(
        ExitSyncState::from_logs(gameprocess, "", APP),
        ExitSyncState::Untracked
    );
}

#[test]
fn only_a_sync_after_the_latest_exit_counts() {
    // A mid-game retry and another app's exit sync.
    let cloud = "\
[2026-11-27 03:22:40] [AppID 2379780] Starting sync (up,)
[2026-11-27 03:22:41] [AppID 2379780] Upload complete in build list
[2026-11-27 03:23:10] [AppID 23797801] Starting sync (up,AC Exit,)
[2026-11-27 03:23:11] [AppID 23797801] Upload complete, result OK
";
    assert_eq!(
        ExitSyncState::from_logs(GAMEPROCESS, cloud, APP),
        ExitSyncState::Exited
    );

    let relaunched = format!(
        "{GAMEPROCESS}[2026-11-27 03:23:20] AppID 2379780 adding PID 15990 as a tracked process\n"
    );
    assert_eq!(
        ExitSyncState::from_logs(&relaunched, CLOUD, APP),
        ExitSyncState::Playing
    );
}

#[test]
fn every_steam_session_end_finishes() {
    for (tags, end) in [
        (
            "up,AC Exit,",
            "Upload failed due to conflicts in build list",
        ),
        (
            "AC Exit,Sync Disabled,",
            "Failed sync for 'AC Exit,Sync Disabled,' [login=false][offlineMode=false]",
        ),
        ("up,", "Job Canceled"),
    ] {
        let cloud = format!(
            "[2026-11-27 03:23:10] [AppID 2379780] Starting sync ({tags})\n\
             [2026-11-27 03:23:11] [AppID 2379780] {end}\n"
        );
        assert_eq!(
            ExitSyncState::from_logs(GAMEPROCESS, &cloud, APP),
            ExitSyncState::Done,
            "{end}"
        );
    }
}

#[test]
fn log_mark_reads_only_new_text() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("cloud_log.txt");
    std::fs::write(&log, "old line\n").unwrap();
    let mark = LogMark::at_end(log.clone());

    let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    f.write_all(b"new line\n").unwrap();
    assert_eq!(mark.appended(), "new line\n");

    // Rotated: shorter than the mark.
    std::fs::write(&log, "rot\n").unwrap();
    assert_eq!(mark.appended(), "rot\n");

    assert_eq!(
        LogMark::at_end(dir.path().join("missing.txt")).appended(),
        ""
    );
}
