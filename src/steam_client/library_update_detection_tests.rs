use super::installed_depots_need_update;
use std::collections::HashMap;

fn map(pairs: &[(u64, u64)]) -> HashMap<u64, u64> {
    pairs.iter().copied().collect()
}

#[test]
fn up_to_date_install_is_not_flagged() {
    // Installed Windows depot matches remote; remote also lists the Linux and
    // macOS builds (not installed). These must NOT trigger a false update.
    let local = map(&[(101, 1111)]);
    let remote = map(&[(101, 1111), (102, 2222), (103, 3333)]);
    assert!(!installed_depots_need_update(&local, &remote));
}

#[test]
fn changed_installed_depot_is_flagged() {
    let local = map(&[(101, 1111)]);
    let remote = map(&[(101, 9999), (102, 2222)]);
    assert!(installed_depots_need_update(&local, &remote));
}

#[test]
fn unknown_remote_depot_for_installed_one_is_ignored() {
    // Remote dropped/renamed the installed depot — nothing to compare, so we
    // don't fabricate an update (and never panic).
    let local = map(&[(101, 1111)]);
    let remote = map(&[(102, 2222)]);
    assert!(!installed_depots_need_update(&local, &remote));
}

#[test]
fn nothing_installed_means_no_update() {
    let remote = map(&[(101, 1111)]);
    assert!(!installed_depots_need_update(&HashMap::new(), &remote));
}
