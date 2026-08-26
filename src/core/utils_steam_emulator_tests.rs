use super::{resolve_steam_emulator, steam_emulator_requested};
use crate::core::config::LauncherConfig;
use crate::core::models::{SteamEmulatorPolicy, UserAppConfig};

fn cfg(global: SteamEmulatorPolicy, path: Option<String>) -> LauncherConfig {
    let mut c = LauncherConfig::default();
    c.steam_emulator = global;
    c.steam_emulator_path = path;
    c
}

fn ua(policy: SteamEmulatorPolicy) -> UserAppConfig {
    let mut u = UserAppConfig::default();
    u.steam_emulator_policy = policy;
    u
}

#[test]
fn default_is_off() {
    let c = LauncherConfig::default();
    assert!(!steam_emulator_requested(None, &c));
    assert!(resolve_steam_emulator(None, &c).is_none());
}

#[test]
fn global_enabled_requests() {
    let c = cfg(SteamEmulatorPolicy::Enabled, None);
    assert!(steam_emulator_requested(None, &c));
}

#[test]
fn per_game_enabled_overrides_global_disabled() {
    let c = cfg(SteamEmulatorPolicy::Disabled, None);
    assert!(steam_emulator_requested(Some(&ua(SteamEmulatorPolicy::Enabled)), &c));
}

#[test]
fn per_game_disabled_overrides_global_enabled() {
    let c = cfg(SteamEmulatorPolicy::Enabled, None);
    assert!(!steam_emulator_requested(Some(&ua(SteamEmulatorPolicy::Disabled)), &c));
}

#[test]
fn resolve_requires_lib_present() {
    let dir = tempfile::tempdir().unwrap();
    let lib = dir.path().join("libsteam_api.so");
    let c = cfg(
        SteamEmulatorPolicy::Enabled,
        Some(lib.to_string_lossy().into_owned()),
    );
    // Enabled but library missing: requested, not resolved.
    assert!(steam_emulator_requested(None, &c));
    assert!(resolve_steam_emulator(None, &c).is_none());
    // Library present: resolved.
    std::fs::write(&lib, b"x").unwrap();
    assert_eq!(resolve_steam_emulator(None, &c), Some(lib));
}
