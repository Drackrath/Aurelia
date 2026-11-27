use super::super::*;
use aurelia::core::config::{ART_CACHE_TTL_SECS, CachedArt};
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
            },
        ),
    ]);
    assert_eq!(art_to_fetch(&[1, 2, 3, 4], &cache, now), vec![2, 4]);
}
