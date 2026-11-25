use super::super::auto_select_platform;
use aurelia::core::models::DepotPlatform::{Linux, Macos, Windows};

#[test]
fn windows_preferred_off_mac() {
    for host in [Linux, Windows] {
        assert_eq!(
            auto_select_platform(&[Windows, Linux, Macos], host),
            Windows
        );
        assert_eq!(auto_select_platform(&[Linux, Macos], host), Linux);
        assert_eq!(auto_select_platform(&[Linux], host), Linux);
        assert_eq!(auto_select_platform(&[Macos], host), Windows);
        assert_eq!(auto_select_platform(&[], host), Windows);
    }
}

#[test]
fn mac_host_prefers_native_build() {
    assert_eq!(auto_select_platform(&[Windows, Linux, Macos], Macos), Macos);
    assert_eq!(auto_select_platform(&[Macos], Macos), Macos);
    assert_eq!(auto_select_platform(&[Windows, Linux], Macos), Windows);
    assert_eq!(auto_select_platform(&[Linux], Macos), Linux);
}
