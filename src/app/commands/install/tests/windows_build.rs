use super::super::windows_build::steam_prefers_linux;
use aurelia::core::models::DepotPlatform::{Linux, Macos, Windows};

#[test]
fn only_windows_builds_of_dual_platform_games() {
    assert!(steam_prefers_linux(
        Linux,
        Windows,
        &[Windows, Linux],
        false
    ));
    // DLC follows the base game.
    assert!(!steam_prefers_linux(
        Linux,
        Windows,
        &[Windows, Linux],
        true
    ));
    // Windows-only: Steam has nothing to swap.
    assert!(!steam_prefers_linux(Linux, Windows, &[Windows], false));
    assert!(!steam_prefers_linux(Linux, Linux, &[Windows, Linux], false));
    // Other hosts never swap.
    assert!(!steam_prefers_linux(
        Windows,
        Windows,
        &[Windows, Linux],
        false
    ));
    assert!(!steam_prefers_linux(
        Macos,
        Windows,
        &[Windows, Linux, Macos],
        false
    ));
}
