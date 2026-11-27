use super::super::*;

fn record(fetched_at: u64, owned_at: u64) -> CachedDlcList {
    CachedDlcList {
        fetched_at,
        owned_at,
        ..Default::default()
    }
}

#[test]
fn fresh_cache_needs_nothing() {
    let need = record(1000, 1000).refresh_needed(1500, 3600 * 24, 3600);
    assert_eq!(
        need,
        DlcRefresh {
            list: false,
            owned: false
        }
    );
}

#[test]
fn stale_ownership_refreshes_only_ownership() {
    let need = record(1000, 1000).refresh_needed(1000 + 3601, 3600 * 24, 3600);
    assert_eq!(
        need,
        DlcRefresh {
            list: false,
            owned: true
        }
    );
}

#[test]
fn stale_list_refreshes_both() {
    let need = record(1000, 90_000).refresh_needed(1000 + 86_401, 86_400, 3600);
    assert_eq!(
        need,
        DlcRefresh {
            list: true,
            owned: true
        }
    );
}

#[test]
fn dlc_cache_round_trips_per_language() {
    // Own dir; env guard serialises tests.
    let tmp = tempfile::tempdir().unwrap();
    let _env =
        crate::test_support::EnvGuard::set(&[("AURELIA_CONFIG_DIR", Some(tmp.path().as_os_str()))]);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut cached = record(5, 6);
        cached.dlc_ids = vec![2778580, 3655690];
        cached.owned = vec![2778580];
        save_dlc_cache(1245620, "english", "DE", &cached)
            .await
            .unwrap();
        let back = load_dlc_cache(1245620, "english", "DE").await.unwrap();
        assert_eq!(back.dlc_ids, cached.dlc_ids);
        assert_eq!(back.owned, cached.owned);
        assert_eq!((back.fetched_at, back.owned_at), (5, 6));
        assert!(load_dlc_cache(1245620, "german", "DE").await.is_none());
    });
}

#[test]
fn ownership_paused_until_retry_after() {
    let mut cached = record(0, 0);
    cached.retry_after = 2000;
    assert!(cached.ownership_paused(1999));
    assert!(!cached.ownership_paused(2000));
}

#[test]
fn old_cache_without_retry_after_parses() {
    let raw = r#"{"fetched_at":1,"dlc_ids":[],"store":[],"owned_at":1,"owned":[]}"#;
    let cached: CachedDlcList = serde_json::from_str(raw).unwrap();
    assert_eq!(cached.retry_after, 0);
}

#[test]
fn other_account_drops_cached_ownership() {
    let mut cached = record(1000, 1000);
    cached.steam_id = Some(1);
    cached.owned = vec![2778580];
    cached.bind_account(Some(2));
    assert!(cached.owned.is_empty());
    assert_eq!(cached.steam_id, Some(2));
    // Ownership now counts as stale.
    assert!(cached.refresh_needed(1000, 86_400, 3600).owned);
    assert!(!cached.refresh_needed(1000, 86_400, 3600).list);
}

#[test]
fn same_account_keeps_cached_ownership() {
    let mut cached = record(1000, 1000);
    cached.steam_id = Some(1);
    cached.owned = vec![2778580];
    cached.bind_account(Some(1));
    assert_eq!(cached.owned, vec![2778580]);
    assert!(!cached.refresh_needed(1000, 86_400, 3600).owned);
}

#[test]
fn zero_ttl_always_refreshes() {
    let need = record(1000, 1000).refresh_needed(1000, 0, 0);
    assert!(need.list && need.owned);
    let need = record(1000, 1000).refresh_needed(1000, 86_400, 0);
    assert!(!need.list && need.owned);
}

#[test]
fn config_without_dlc_cache_keys_uses_defaults() {
    let mut old = serde_json::to_value(LauncherConfig::default()).unwrap();
    let obj = old.as_object_mut().unwrap();
    obj.remove("dlc_list_cache_secs");
    obj.remove("dlc_owned_cache_secs");
    let cfg: LauncherConfig = serde_json::from_value(old).unwrap();
    assert_eq!(cfg.dlc_list_cache_secs, DLC_LIST_TTL_SECS);
    assert_eq!(cfg.dlc_owned_cache_secs, DLC_OWNED_TTL_SECS);
}
