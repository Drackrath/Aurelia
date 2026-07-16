use super::*;

#[test]
fn ignores_tooling_by_app_id() {
    assert!(is_ignored_steam_app(228980, "Steamworks Common Redistributables"));
    assert!(is_ignored_steam_app(1628350, "")); // Steam Linux Runtime 3.0
    assert!(is_ignored_steam_app(1493710, "Proton Experimental"));
}

#[test]
fn ignores_tooling_by_name_prefix() {
    // App id not in the list, but the name marks it as tooling.
    assert!(is_ignored_steam_app(9999999, "Proton 9.0 (Beta)"));
    assert!(is_ignored_steam_app(9999998, "  Steam Linux Runtime 4.0"));
}

#[test]
fn keeps_real_games() {
    assert!(!is_ignored_steam_app(620, "Portal 2"));
    // A game that merely contains "Proton" mid-name is not tooling.
    assert!(!is_ignored_steam_app(12345, "The Protonist"));
}

#[test]
fn build_game_library_filters_tooling() {
    let owned = vec![
        OwnedGame {
            app_id: 620,
            name: "Portal 2".to_string(),
            playtime_forever_minutes: 0,
            local_manifest_ids: HashMap::new(),
            update_available: false,
        },
        OwnedGame {
            app_id: 228980,
            name: "Steamworks Common Redistributables".to_string(),
            playtime_forever_minutes: 0,
            local_manifest_ids: HashMap::new(),
            update_available: false,
        },
    ];
    let lib = build_game_library(owned, HashMap::new(), None);
    assert_eq!(lib.games.len(), 1);
    assert_eq!(lib.games[0].app_id, 620);
}
