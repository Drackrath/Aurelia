use super::super::*;

use std::collections::HashMap;

#[test]
fn new_default_entry_is_dropped() {
    let mut store: HashMap<u32, u8> = HashMap::from([(1, 0)]);
    assert!(!settle_entry(&mut store, 1, None));
    assert!(store.is_empty());
}

#[test]
fn stale_default_entry_is_cleaned_up() {
    let mut store: HashMap<u32, u8> = HashMap::from([(1, 0)]);
    assert!(settle_entry(&mut store, 1, Some(0)));
    assert!(store.is_empty());
}

#[test]
fn real_changes_are_reported() {
    let mut store: HashMap<u32, u8> = HashMap::from([(1, 2)]);
    assert!(settle_entry(&mut store, 1, None));
    assert!(settle_entry(&mut store, 1, Some(5)));
    store.remove(&1);
    assert!(settle_entry(&mut store, 1, Some(2)));
}
