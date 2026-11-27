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

#[tokio::test]
async fn dlc_cache_round_trips_per_language() {
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
}
