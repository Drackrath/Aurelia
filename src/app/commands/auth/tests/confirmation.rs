use super::super::*;
use aurelia::core::models::SteamGuardReq;

#[test]
fn confirmation_methods_match_guard_required_types() {
    let email = SteamGuardReq::EmailCode {
        domain_hint: "gmail.com".to_string(),
    };
    assert_eq!(confirmation_method_name(&email), "email");
    assert_eq!(
        confirmation_method_name(&SteamGuardReq::DeviceCode),
        "device"
    );
    assert_eq!(
        confirmation_method_name(&SteamGuardReq::DeviceConfirmation),
        "device_confirmation"
    );
}
