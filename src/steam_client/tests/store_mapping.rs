use super::super::*;

#[test]
fn store_app_type_labels() {
    assert_eq!(
        store_app_type_label(EStoreAppType::k_EStoreAppType_Game),
        "Game"
    );
    assert_eq!(
        store_app_type_label(EStoreAppType::k_EStoreAppType_DLC),
        "DLC"
    );
    assert_eq!(
        store_app_type_label(EStoreAppType::k_EStoreAppType_Music),
        "Soundtrack"
    );
}

fn item_with_assets(fmt: &str, header: &str, small: &str) -> StoreItem {
    use steam_vent_proto::steammessages_storebrowse_steamclient::store_item::Assets;
    let mut assets = Assets::new();
    assets.set_asset_url_format(fmt.to_string());
    assets.set_header(header.to_string());
    assets.set_small_capsule(small.to_string());
    let mut item = StoreItem::new();
    item.set_appid(3655690);
    item.assets = MessageField::some(assets);
    item
}

#[test]
fn hashed_asset_paths_win_over_flat_cdn() {
    let item = item_with_assets(
        "steam/apps/3655690/${FILENAME}?t=1788321529",
        "8ecd71d85e597e7616e5b36e697c0c018fb65f5e/header.jpg",
        "c6fd066656060ea1d8bd540873beb90762dafdc9/capsule_231x87.jpg",
    );
    let assets = store_assets(&item);
    assert_eq!(
        assets.header.as_deref(),
        Some(
            "https://shared.steamstatic.com/store_item_assets/steam/apps/3655690/8ecd71d85e597e7616e5b36e697c0c018fb65f5e/header.jpg?t=1788321529"
        )
    );
    assert_eq!(
        assets.small_capsule.as_deref(),
        Some(
            "https://shared.steamstatic.com/store_item_assets/steam/apps/3655690/c6fd066656060ea1d8bd540873beb90762dafdc9/capsule_231x87.jpg?t=1788321529"
        )
    );
}

#[test]
fn missing_assets_fall_back_to_flat_cdn() {
    let mut item = StoreItem::new();
    item.set_appid(10);
    let assets = store_assets(&item);
    assert_eq!(
        assets.small_capsule.as_deref(),
        Some("https://cdn.cloudflare.steamstatic.com/steam/apps/10/capsule_231x87.jpg")
    );
}

#[test]
fn store_art_reads_music_type_and_hashed_assets() {
    let mut item = item_with_assets(
        "steam/apps/2387900/${FILENAME}",
        "abc/header.jpg",
        "abc/capsule_231x87.jpg",
    );
    item.set_type(EStoreAppType::k_EStoreAppType_Music);
    let art = store_art(&item);
    assert_eq!(art.app_type.as_deref(), Some("Soundtrack"));
    assert!(art.assets.is_some());
}

#[test]
fn store_art_without_type_or_format_stays_empty() {
    let mut item = StoreItem::new();
    item.set_appid(10);
    let art = store_art(&item);
    // Unset type is not Game.
    assert_eq!(art.app_type, None);
    assert_eq!(art.assets, None);
}

#[test]
fn store_art_captures_status_fields() {
    let mut item = StoreItem::new();
    item.set_id(353130);
    item.set_appid(353130);
    item.set_success(1);
    item.set_visible(true);
    item.set_unlisted(true);
    item.set_name("Time of Dragons".to_string());
    item.set_store_url_path("app/353130/Time_of_Dragons".to_string());
    let status = store_art(&item).status;
    assert!(status.unlisted && status.visible);
    assert_eq!(status.success, 1);
    assert_eq!(status.store_url_path, "app/353130/Time_of_Dragons");
    assert_eq!(status.best_purchase, None);
    assert!(!status.is_delisted());
}

#[test]
fn removed_item_reads_as_removed() {
    // Firefall: id echoed, appid zero.
    let mut item = StoreItem::new();
    item.set_id(227700);
    item.set_success(15);
    let status = store_art(&item).status;
    assert_eq!(status.appid, 0);
    assert_eq!(status.availability_label(), Some("Removed"));
}
