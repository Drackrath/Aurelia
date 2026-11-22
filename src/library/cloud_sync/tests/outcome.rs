use super::super::*;

#[test]
fn skipped_files_are_reported_not_dropped() {
    // An unresolvable file must be visible in the outcome; a bare "0 downloaded"
    // is indistinguishable from an empty cloud.
    let outcome = SyncOutcome {
        skipped: vec![
            SyncSkip {
                filename: "%WinAppDataLocal%a.sav".to_string(),
                root_token: Some("WinAppDataLocal".to_string()),
                reason: "unsupported".to_string(),
            },
            SyncSkip {
                filename: "%WinAppDataLocal%b.sav".to_string(),
                root_token: Some("WinAppDataLocal".to_string()),
                reason: "unsupported".to_string(),
            },
            SyncSkip {
                filename: "%FutureRoot%c.sav".to_string(),
                root_token: Some("FutureRoot".to_string()),
                reason: "unsupported".to_string(),
            },
        ],
        ..Default::default()
    };
    assert!(outcome.has_skips());
    assert!(!outcome.has_conflicts());
    // Deduplicated: two files sharing a root are one problem, not two.
    assert_eq!(
        outcome.skipped_tokens(),
        vec!["FutureRoot".to_string(), "WinAppDataLocal".to_string()],
    );
}

#[test]
fn a_failed_transfer_is_recorded_and_marks_the_run_incomplete() {
    // The case that left a save set half-restored: one file failing must not be
    // readable as success, because the game will start on the partial set.
    let mut outcome = SyncOutcome {
        downloaded: vec!["a.sav".to_string()],
        ..Default::default()
    };
    outcome.record_failure(
        "%WinAppDataLocal%EXPEDITION_0.sav",
        "down",
        &anyhow::anyhow!("Response timed out"),
    );

    assert!(outcome.has_failures());
    assert!(outcome.has_failures() || outcome.has_skips());
    assert_eq!(outcome.failed[0].direction, "down");
    assert!(outcome.failed[0].error.contains("timed out"));
    // The successful transfer is still reported — partial progress is kept.
    assert_eq!(outcome.downloaded, vec!["a.sav".to_string()]);
}

#[test]
fn a_clean_run_has_no_failures_or_skips() {
    let outcome = SyncOutcome {
        downloaded: vec!["a.sav".to_string()],
        ..Default::default()
    };
    assert!(!outcome.has_skips());
    assert!(!outcome.has_failures());
}
