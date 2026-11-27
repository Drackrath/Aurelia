use super::super::*;

#[test]
fn art_goes_stale_after_ttl() {
    let art = CachedArt {
        fetched_at: 100,
        assets: None,
        app_type: None,
    };
    assert!(!art.is_stale(100 + ART_CACHE_TTL_SECS - 1, ART_CACHE_TTL_SECS));
    assert!(art.is_stale(100 + ART_CACHE_TTL_SECS, ART_CACHE_TTL_SECS));
}

#[test]
fn art_cache_round_trips_known_and_unknown_apps() {
    let tmp = tempfile::tempdir().unwrap();
    let _env =
        crate::test_support::EnvGuard::set(&[("AURELIA_CONFIG_DIR", Some(tmp.path().as_os_str()))]);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let cache = HashMap::from([
            (
                4030830,
                CachedArt {
                    fetched_at: 5,
                    assets: Some(crate::steam_client::StoreAppAssets {
                        header: Some("https://example/hashed/header.jpg".into()),
                        ..Default::default()
                    }),
                    app_type: Some("Soundtrack".into()),
                },
            ),
            (
                10,
                CachedArt {
                    fetched_at: 6,
                    assets: None,
                    app_type: None,
                },
            ),
        ]);
        save_art_cache(&cache).await.unwrap();
        assert_eq!(load_art_cache().await, cache);
    });
}

#[test]
fn art_cached_before_types_is_refetched_once() {
    let mut art = CachedArt {
        fetched_at: 100,
        assets: Some(crate::steam_client::StoreAppAssets::default()),
        app_type: None,
    };
    assert!(art.is_stale(101, ART_CACHE_TTL_SECS));
    art.app_type = Some("Game".into());
    assert!(!art.is_stale(101, ART_CACHE_TTL_SECS));
}

#[test]
fn artless_untyped_app_is_not_refetched() {
    let art = CachedArt {
        fetched_at: 100,
        assets: None,
        app_type: None,
    };
    assert!(!art.is_stale(101, ART_CACHE_TTL_SECS));
}

#[test]
fn old_cache_entry_without_type_parses() {
    let art: CachedArt = serde_json::from_str(r#"{"fetched_at":1,"assets":null}"#).unwrap();
    assert_eq!(art.app_type, None);
}
