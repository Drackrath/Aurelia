use super::{installed_depots_need_update, update_state};
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

#[test]
fn flag_with_current_depots_is_a_config_mismatch() {
    let local = map(&[(2378505, 1111)]);
    let remote = map(&[(2378503, 2222), (2378505, 1111)]);
    assert_eq!(update_state(&local, &remote, true), (false, true));
}

#[test]
fn flag_with_newer_remote_is_an_update() {
    let local = map(&[(101, 1111)]);
    let remote = map(&[(101, 9999)]);
    assert_eq!(update_state(&local, &remote, true), (true, false));
}

#[test]
fn flag_on_staged_install_without_depots_stays_an_update() {
    let remote = map(&[(101, 1111)]);
    assert_eq!(update_state(&HashMap::new(), &remote, true), (true, false));
}

#[test]
fn no_flag_and_current_depots_is_neither() {
    let local = map(&[(101, 1111)]);
    assert_eq!(update_state(&local, &local.clone(), false), (false, false));
}
