use super::*;
use crate::core::config::{GameConfig, LauncherConfig};
use std::collections::HashMap;
use tempfile::tempdir;

fn config_with_script(app_id: u32, script: Option<&str>) -> LauncherConfig {
    let mut cfg = LauncherConfig::default();
    let mut gc = GameConfig::default();
    gc.launch_script = script.map(|s| s.to_string());
    let mut map = HashMap::new();
    map.insert(app_id, gc);
    cfg.game_configs = map;
    cfg
}

#[test]
fn disabled_returns_none_even_with_override() {
    let tmp = tempdir().unwrap();
    let script = tmp.path().join("custom.sh");
    std::fs::write(&script, "exec \"$@\"\n").unwrap();
    let cfg = config_with_script(42, Some(script.to_string_lossy().as_ref()));
    assert!(resolve(42, Some(&cfg), Some(&script), true).is_none());
}

#[test]
fn override_beats_config() {
    let over = PathBuf::from("/some/override.sh");
    let cfg = config_with_script(42, Some("/config/pinned.sh"));
    let got = resolve(42, Some(&cfg), Some(&over), false);
    assert_eq!(got, Some(over));
}

#[test]
fn config_beats_dir() {
    // No override; a config-pinned path is returned (even if missing) ahead of
    // any auto-detected dir script.
    let cfg = config_with_script(42, Some("/config/pinned.sh"));
    let got = resolve(42, Some(&cfg), None, false);
    assert_eq!(got, Some(PathBuf::from("/config/pinned.sh")));
}

#[test]
fn dir_script_used_when_present() {
    let _guard = SCRIPT_DIR_ENV_LOCK.lock().unwrap();
    let tmp = tempdir().unwrap();
    // Point AURELIA_SCRIPT_DIR at a temp dir with an on-disk script.
    unsafe { std::env::set_var("AURELIA_SCRIPT_DIR", tmp.path()) };
    let path = dir_script_path(4242);
    std::fs::write(&path, "exec \"$@\"\n").unwrap();

    let got = resolve(4242, None, None, false);
    assert_eq!(got, Some(path));

    // No script for a different id.
    assert!(resolve(9999, None, None, false).is_none());
    unsafe { std::env::remove_var("AURELIA_SCRIPT_DIR") };
}

#[test]
fn empty_config_script_is_ignored() {
    let _guard = SCRIPT_DIR_ENV_LOCK.lock().unwrap();
    let cfg = config_with_script(42, Some(""));
    // Empty string is treated as unset; with no override and no dir script -> None.
    let tmp = tempdir().unwrap();
    unsafe { std::env::set_var("AURELIA_SCRIPT_DIR", tmp.path()) };
    let got = resolve(42, Some(&cfg), None, false);
    assert!(got.is_none());
    unsafe { std::env::remove_var("AURELIA_SCRIPT_DIR") };
}

#[test]
#[cfg(not(windows))]
fn template_unix_has_shebang_and_passthrough() {
    let t = template(1234, "Test Game");
    assert!(t.starts_with("#!/usr/bin/env bash"), "missing shebang: {t}");
    assert!(t.contains("\"$@\""), "missing \"$@\": {t}");
    assert!(t.contains("1234"));
    assert!(t.contains("AURELIA_APP_ID"));
}

#[test]
#[cfg(windows)]
fn template_windows_has_passthrough() {
    let t = template(1234, "Test Game");
    assert!(t.contains("%*"), "missing %*: {t}");
    assert!(t.contains("%AURELIA_LAUNCH_PROGRAM%"), "missing passthrough: {t}");
    assert!(t.contains("1234"));
    assert!(t.contains("AURELIA_APP_ID"));
}

#[test]
fn filename_is_platform_specific() {
    let f = script_filename(77);
    if cfg!(windows) {
        assert_eq!(f, "77.bat");
    } else {
        assert_eq!(f, "77.sh");
    }
}
