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
