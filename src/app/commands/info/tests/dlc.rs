use super::super::*;
use aurelia::core::models::DlcState;

fn store(name: &str) -> StoreAppInfo {
    StoreAppInfo {
        app_id: 1,
        name: name.to_string(),
        ..Default::default()
    }
}

fn state(owned: bool, installed: bool, disabled: bool) -> DlcState {
    DlcState {
        app_id: 1,
        owned,
        installed,
        disabled,
    }
}

#[test]
fn dlc_without_store_item_is_retired() {
    assert!(dlc_is_retired(None));
}

#[test]
fn dlc_with_nameless_store_item_is_retired() {
    assert!(dlc_is_retired(Some(&store("  "))));
}

#[test]
fn dlc_with_named_store_item_is_live() {
    assert!(!dlc_is_retired(Some(&store("Shadow of the Erdtree"))));
}

#[test]
fn retired_unowned_dlc_is_hidden() {
    assert!(!dlc_is_listed(
        true,
        Some(&state(false, false, false)),
        false
    ));
    assert!(!dlc_is_listed(true, None, false));
}

#[test]
fn retired_dlc_the_user_has_stays_listed() {
    assert!(dlc_is_listed(true, Some(&state(true, false, false)), false));
    assert!(dlc_is_listed(true, Some(&state(false, true, false)), false));
    assert!(dlc_is_listed(true, Some(&state(false, false, true)), false));
}

#[test]
fn all_lists_retired_dlc() {
    assert!(dlc_is_listed(true, None, true));
}

#[test]
fn live_dlc_is_always_listed() {
    assert!(dlc_is_listed(false, None, false));
}

#[test]
fn dlc_image_prefers_store_assets() {
    let mut info = store("Tarnished Pack");
    info.assets.header = Some("https://example/hashed/header.jpg".into());
    info.assets.small_capsule = Some("https://example/hashed/capsule.jpg".into());
    assert_eq!(
        dlc_image(Some(&info), 3655690),
        "https://example/hashed/header.jpg"
    );
    assert_eq!(
        dlc_fallback_image(Some(&info), 3655690),
        "https://example/hashed/capsule.jpg"
    );
}

#[test]
fn dlc_image_without_store_item_uses_cdn() {
    assert_eq!(dlc_image(None, 10), steam_urls::header_url(10));
    assert_eq!(
        dlc_fallback_image(None, 10),
        steam_urls::small_capsule_url(10)
    );
}
