use super::super::*;

fn cloud(hash: &str, ts: u64) -> CloudFileEntry {
    CloudFileEntry {
        filename: "save.dat".to_string(),
        timestamp: ts,
        size: 10,
        sha_hash: Some(hash.to_string()),
    }
}
fn local(hash: &str, ts: u64) -> LocalInfo {
    LocalInfo {
        hash: hash.to_string(),
        size: 10,
        timestamp: ts,
    }
}
fn base(hash: &str) -> BaselineEntry {
    BaselineEntry {
        hash: hash.to_string(),
        timestamp: 0,
        size: 10,
    }
}

#[test]
fn identical_content_is_skipped_regardless_of_timestamp() {
    // Same bytes on both sides but different mtimes must NOT cause a transfer.
    let action = plan_action(None, Some(&local("aaaa", 200)), Some(&cloud("AAAA", 100)));
    assert_eq!(
        action,
        PlannedAction::Skip,
        "hash match wins over timestamps"
    );
}

#[test]
fn only_local_changed_uploads_not_conflicts() {
    // The normal play loop: baseline==cloud, the user played so local advanced.
    // This must be a plain upload, never a conflict prompt.
    let action = plan_action(
        Some(&base("cccc")),
        Some(&local("llll", 300)),
        Some(&cloud("cccc", 100)),
    );
    assert_eq!(action, PlannedAction::Upload);
}

#[test]
fn only_cloud_changed_downloads() {
    // Played on another machine: baseline==local, cloud advanced. Plain download.
    let action = plan_action(
        Some(&base("llll")),
        Some(&local("llll", 100)),
        Some(&cloud("cccc", 300)),
    );
    assert_eq!(action, PlannedAction::Download);
}

#[test]
fn both_changed_is_a_conflict() {
    // Both sides diverged from the last-synced baseline — the data-loss case.
    let action = plan_action(
        Some(&base("oldd")),
        Some(&local("llll", 300)),
        Some(&cloud("cccc", 290)),
    );
    assert_eq!(action, PlannedAction::Conflict);
}

#[test]
fn first_sync_with_two_differing_copies_is_a_conflict() {
    // No baseline yet and the copies differ: we can't know which lineage wins.
    let action = plan_action(None, Some(&local("llll", 300)), Some(&cloud("cccc", 100)));
    assert_eq!(action, PlannedAction::Conflict);
}

#[test]
fn one_sided_presence_moves_the_only_way_it_can() {
    assert_eq!(
        plan_action(None, Some(&local("llll", 1)), None),
        PlannedAction::Upload
    );
    assert_eq!(
        plan_action(None, None, Some(&cloud("cccc", 1))),
        PlannedAction::Download
    );
}
