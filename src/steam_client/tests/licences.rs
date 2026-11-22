use super::super::*;

fn licence_game(is_owned: bool, is_family_shared: bool) -> LibraryGame {
    LibraryGame {
        app_id: 2795000,
        name: "Free Game".to_string(),
        install_path: None,
        is_installed: true,
        playtime_forever_minutes: None,
        active_branch: "public".to_string(),
        update_available: false,
        steam_config_mismatch: false,
        update_queued: false,
        local_manifest_ids: HashMap::new(),
        is_owned,
        is_family_shared,
        online_required: None,
        platform: None,
        from_windows_steam: false,
        size_on_disk: None,
    }
}

#[test]
fn steam_is_forced_only_for_borrowed_or_unconfirmed_licences() {
    assert_eq!(forced_steam_reason(&licence_game(true, false)), None);
    assert_eq!(
        forced_steam_reason(&licence_game(false, true)),
        Some("family-shared")
    );
    assert_eq!(
        forced_steam_reason(&licence_game(true, true)),
        Some("family-shared")
    );
    assert_eq!(
        forced_steam_reason(&licence_game(false, false)),
        Some("licence-unconfirmed")
    );
}

#[test]
fn owned_games_request_includes_free_licences() {
    let request = library::owned_games_request(76561198000000001);
    assert_eq!(request.steamid(), 76561198000000001);
    assert!(request.include_appinfo());
    assert!(request.include_played_free_games());
    assert!(request.include_free_sub());
}
