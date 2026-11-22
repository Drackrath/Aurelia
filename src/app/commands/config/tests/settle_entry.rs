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

#[test]
fn unknown_only_entries_are_kept() {
    use aurelia::core::config::GameConfig;
    use aurelia::core::models::UserAppConfig;

    let mut game = GameConfig::default();
    game.extra
        .insert("aureliaworks_override".into(), "steamless".into());
    let mut games = HashMap::from([(1, game.clone())]);
    assert!(!settle_entry(&mut games, 1, Some(game.clone())));
    assert_eq!(games.get(&1), Some(&game));

    let mut app = UserAppConfig::default();
    app.extra.insert("aureliaworks_runtime".into(), 1.into());
    let mut apps = HashMap::from([(1, app.clone())]);
    assert!(!settle_entry(&mut apps, 1, Some(app.clone())));
    assert_eq!(apps.get(&1), Some(&app));
}
