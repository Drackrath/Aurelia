use super::super::*;

#[test]
fn art_goes_stale_after_ttl() {
    let art = CachedArt {
        fetched_at: 100,
        assets: None,
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
                },
            ),
            (
                10,
                CachedArt {
                    fetched_at: 6,
                    assets: None,
                },
            ),
        ]);
        save_art_cache(&cache).await.unwrap();
        assert_eq!(load_art_cache().await, cache);
    });
}
