use super::super::*;
use aurelia::core::config::{ART_CACHE_TTL_SECS, ART_CACHE_VERSION, CachedArt};
use aurelia::steam_client::StoreAppAssets;
use std::collections::HashMap;

const HASHED: &str =
    "https://shared.steamstatic.com/store_item_assets/steam/apps/4030830/20e2394f/header.jpg";

fn hashed(fetched_at: u64) -> CachedArt {
    CachedArt {
        fetched_at,
        assets: Some(StoreAppAssets {
            header: Some(HASHED.to_string()),
            ..Default::default()
        }),
        app_type: Some("Game".to_string()),
        status: None,
        version: ART_CACHE_VERSION,
    }
}

#[test]
fn hashed_header_replaces_the_flat_path() {
    let art = library_art(4030830, Some(&hashed(1)));
    assert_eq!(art["header"], HASHED);
    // Missing fields stay flat.
    assert_eq!(art["capsule"], steam_urls::capsule_url(4030830));
    assert_eq!(art["logo"], steam_urls::logo_url(4030830));
}

#[test]
fn unknown_or_artless_apps_keep_flat_paths() {
    let none = CachedArt {
        fetched_at: 1,
        assets: None,
        app_type: None,
        status: None,
        version: ART_CACHE_VERSION,
    };
    for cached in [None, Some(&none)] {
        let art = library_art(10, cached);
        assert_eq!(art["header"], steam_urls::header_url(10));
        assert_eq!(art["hero"], steam_urls::hero_url(10));
    }
}

#[test]
fn only_missing_or_stale_art_is_fetched() {
    let now = 10 * ART_CACHE_TTL_SECS;
    let cache = HashMap::from([
        (1, hashed(now)),
        (2, hashed(now - ART_CACHE_TTL_SECS)),
        (
            3,
            CachedArt {
                fetched_at: now,
                assets: None,
                app_type: None,
                status: None,
                version: ART_CACHE_VERSION,
            },
        ),
    ]);
    assert_eq!(art_to_fetch(&[1, 2, 3, 4], &cache, now), vec![2, 4]);
}

fn installed_at(path: &str) -> aurelia::core::models::LibraryGame {
    aurelia::core::models::LibraryGame {
        app_id: 2387900,
        name: "Lords of the Fallen Soundtrack".to_string(),
        install_path: Some(path.to_string()),
        is_installed: true,
        playtime_forever_minutes: None,
        active_branch: "public".to_string(),
        update_available: false,
        steam_config_mismatch: false,
        update_queued: false,
        local_manifest_ids: HashMap::new(),
        is_owned: true,
        is_family_shared: false,
        online_required: None,
        platform: None,
        from_windows_steam: false,
        size_on_disk: None,
    }
}

#[test]
fn store_type_wins() {
    let game = installed_at("/lib/steamapps/common/Game");
    let mut art = hashed(1);
    art.app_type = Some("Soundtrack".to_string());
    assert_eq!(
        library_app_type(&game, Some(&art)).as_deref(),
        Some("Soundtrack")
    );
}

#[test]
fn music_folder_means_soundtrack_without_store_type() {
    let game =
        installed_at("/home/u/.local/share/Steam/steamapps/music/Lords of the Fallen - Soundtrack");
    assert_eq!(library_app_type(&game, None).as_deref(), Some("Soundtrack"));
    let game = installed_at("/lib/steamapps/common/Lords of the Fallen");
    assert_eq!(library_app_type(&game, None), None);
}

#[test]
fn availability_wins_over_store_type() {
    let game = installed_at("/lib/steamapps/music/OST");
    let mut art = hashed(1);
    art.app_type = Some("Soundtrack".to_string());
    art.status = Some(aurelia::steam_client::StoreStatus {
        success: 15,
        ..Default::default()
    });
    assert_eq!(
        library_app_type(&game, Some(&art)).as_deref(),
        Some("Removed")
    );
    art.status = Some(aurelia::steam_client::StoreStatus {
        success: 15,
        appid: 2387900,
        name: "OST".to_string(),
        ..Default::default()
    });
    assert_eq!(
        library_app_type(&game, Some(&art)).as_deref(),
        Some("Delisted")
    );
    art.status = Some(aurelia::steam_client::StoreStatus {
        visible: true,
        ..Default::default()
    });
    assert_eq!(
        library_app_type(&game, Some(&art)).as_deref(),
        Some("Soundtrack")
    );
}
