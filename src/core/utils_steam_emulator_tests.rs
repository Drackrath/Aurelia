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
    assert!(!steam_emulator_requested(None, &c, None));
    assert!(resolve_steam_emulator(None, &c, None).is_none());
}

#[test]
fn global_enabled_requests() {
    let c = cfg(SteamEmulatorPolicy::Enabled, None);
    assert!(steam_emulator_requested(None, &c, None));
}

#[test]
fn per_game_enabled_overrides_global_disabled() {
    let c = cfg(SteamEmulatorPolicy::Disabled, None);
    assert!(steam_emulator_requested(Some(&ua(SteamEmulatorPolicy::Enabled)), &c, None));
}

#[test]
fn per_game_disabled_overrides_global_enabled() {
    let c = cfg(SteamEmulatorPolicy::Enabled, None);
    assert!(!steam_emulator_requested(Some(&ua(SteamEmulatorPolicy::Disabled)), &c, None));
}

#[test]
fn online_required_blocks_emulation() {
    let c = cfg(SteamEmulatorPolicy::Enabled, None);
    let enabled = ua(SteamEmulatorPolicy::Enabled);
    // Online-required beats every enablement path.
    assert!(!steam_emulator_requested(None, &c, Some(true)));
    assert!(!steam_emulator_requested(Some(&enabled), &c, Some(true)));
    assert!(resolve_steam_emulator(Some(&enabled), &c, Some(true)).is_none());
    // Offline-capable and unknown stay allowed.
    assert!(steam_emulator_requested(None, &c, Some(false)));
    assert!(steam_emulator_requested(None, &c, None));
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
    assert!(steam_emulator_requested(None, &c, None));
    assert!(resolve_steam_emulator(None, &c, None).is_none());
    // Library present: resolved.
    std::fs::write(&lib, b"x").unwrap();
    assert_eq!(resolve_steam_emulator(None, &c, None), Some(lib));
}
