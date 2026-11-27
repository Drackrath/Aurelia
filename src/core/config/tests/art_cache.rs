use super::super::*;

#[test]
fn art_goes_stale_after_ttl() {
    let art = CachedArt {
        fetched_at: 100,
        assets: None,
        app_type: None,
        status: None,
        version: ART_CACHE_VERSION,
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
                    status: Some(crate::steam_client::StoreStatus {
                        success: 1,
                        appid: 4030830,
                        name: "OST".into(),
                        ..Default::default()
                    }),
                    version: ART_CACHE_VERSION,
                },
            ),
            (
                10,
                CachedArt {
                    fetched_at: 6,
                    assets: None,
                    app_type: None,
                    status: None,
                    version: ART_CACHE_VERSION,
                },
            ),
        ]);
        save_art_cache(&cache).await.unwrap();
        assert_eq!(load_art_cache().await, cache);
    });
}

#[test]
fn older_cache_layout_is_refetched_once() {
    let mut art = CachedArt {
        fetched_at: 100,
        assets: Some(crate::steam_client::StoreAppAssets::default()),
        app_type: Some("Game".into()),
        status: None,
        version: 0,
    };
    assert!(art.is_stale(101, ART_CACHE_TTL_SECS));
    art.version = ART_CACHE_VERSION;
    assert!(!art.is_stale(101, ART_CACHE_TTL_SECS));
}

#[test]
fn artless_untyped_app_is_not_refetched() {
    let art = CachedArt {
        fetched_at: 100,
        assets: None,
        app_type: None,
        status: None,
        version: ART_CACHE_VERSION,
    };
    assert!(!art.is_stale(101, ART_CACHE_TTL_SECS));
}

#[test]
fn old_cache_entry_without_type_parses() {
    let art: CachedArt = serde_json::from_str(r#"{"fetched_at":1,"assets":null}"#).unwrap();
    assert_eq!(art.app_type, None);
    assert_eq!(art.status, None);
    assert_eq!(art.version, 0);
}

fn status(f: impl FnOnce(&mut crate::steam_client::StoreStatus)) -> CachedArt {
    let mut s = crate::steam_client::StoreStatus {
        success: 1,
        visible: true,
        appid: 353130,
        name: "Time of Dragons".into(),
        ..Default::default()
    };
    f(&mut s);
    CachedArt {
        fetched_at: 1,
        assets: None,
        app_type: Some("Game".into()),
        status: Some(s),
        version: ART_CACHE_VERSION,
    }
}

#[test]
fn visible_item_has_no_label() {
    assert_eq!(status(|_| {}).availability_label(), None);
    // Unlisted alone stays listed.
    assert_eq!(status(|s| s.unlisted = true).availability_label(), None);
}

#[test]
fn removed_needs_all_three_signals() {
    let removed = status(|s| {
        s.visible = false;
        s.success = 15;
        s.appid = 0;
        s.name.clear();
    });
    assert_eq!(removed.availability_label(), Some("Removed"));
}

#[test]
fn hidden_named_item_is_delisted() {
    // Time of Dragons in DE.
    let hidden = status(|s| {
        s.visible = false;
        s.success = 15;
    });
    assert_eq!(hidden.availability_label(), Some("Delisted"));
}

#[test]
fn missing_status_has_no_label() {
    let mut art = status(|_| {});
    art.status = None;
    assert_eq!(art.availability_label(), None);
}
