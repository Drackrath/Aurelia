use super::super::*;

fn cats(pairs: &[(u32, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(id, v)| (format!("category_{id}"), v.to_string()))
        .collect()
}

#[test]
fn online_required_mmo_without_single_player() {
    // MMO (20), no single-player => requires online.
    assert!(category_online_required(&cats(&[(20, "1"), (1, "1")])));
}

#[test]
fn online_required_online_coop_without_single_player() {
    // Online Co-op (38) only => requires online.
    assert!(category_online_required(&cats(&[(38, "1")])));
}

#[test]
fn not_online_required_when_single_player_present() {
    // Online PvP (36) but also Single-player (2) => playable offline.
    assert!(!category_online_required(&cats(&[(36, "1"), (2, "1")])));
}

#[test]
fn not_online_required_for_local_multiplayer_only() {
    // Generic Multi-player (1) / Shared-Split-Screen (24) are not online-only.
    assert!(!category_online_required(&cats(&[(1, "1"), (24, "1")])));
}

#[test]
fn not_online_required_when_categories_absent_or_zeroed() {
    assert!(!category_online_required(&cats(&[])));
    assert!(!category_online_required(&cats(&[(20, "0"), (2, "0")])));
}
