use super::*;

fn config(on: bool, auto: bool) -> LauncherConfig {
    LauncherConfig {
        steamworks_client: on,
        steamworks_client_auto: auto,
        ..LauncherConfig::default()
    }
}

#[test]
fn decision_table() {
    // on, auto, running, installed -> choice
    let rows = [
        (false, false, false, true, Choice::Off),
        (true, false, true, false, Choice::On),
        (true, true, true, true, Choice::On),
        (false, true, true, true, Choice::SteamRunning),
        (false, true, false, true, Choice::StandIn),
        (false, true, false, false, Choice::NotInstalled),
        (false, true, true, false, Choice::SteamRunning),
    ];
    for (on, auto, running, installed, want) in rows {
        let lib = if installed {
            Library::Ready
        } else {
            Library::Missing
        };
        let got = decide(&config(on, auto), || running, || lib);
        assert_eq!(
            got, want,
            "on={on} auto={auto} running={running} installed={installed}"
        );
    }
}

#[test]
fn detectors_run_only_in_auto() {
    let never = || -> bool { panic!("detector ran") };
    let no_lib = || -> Library { panic!("detector ran") };
    assert_eq!(decide(&config(false, false), never, no_lib), Choice::Off);
    assert_eq!(decide(&config(true, false), never, no_lib), Choice::On);
    // Steam running: no install check.
    assert_eq!(
        decide(&config(false, true), || true, no_lib),
        Choice::SteamRunning
    );
}

#[test]
fn apply_sets_the_launch_flag_only() {
    let mut cfg = config(false, true);
    assert_eq!(
        apply(&mut cfg, || false, |_| Library::Ready),
        Choice::StandIn
    );
    assert!(cfg.steamworks_client && cfg.steamworks_client_auto);
    let mut cfg = config(false, true);
    let got = apply(&mut cfg, || true, |_| Library::Ready);
    assert_eq!(got, Choice::SteamRunning);
    assert!(!cfg.steamworks_client);
    let mut cfg = config(true, false);
    assert_eq!(apply(&mut cfg, || true, |_| Library::Missing), Choice::On);
    assert!(cfg.steamworks_client);
    // Bad hash: normal launch.
    let mut cfg = config(false, true);
    let got = apply(&mut cfg, || false, |_| Library::Unverified);
    assert_eq!(got, Choice::Unverified);
    assert!(!cfg.steamworks_client);
}

#[test]
fn notes_name_the_path_and_reason() {
    assert!(Choice::StandIn.note().unwrap().starts_with("stand-in:"));
    assert!(Choice::SteamRunning.note().unwrap().contains("running"));
    assert!(Choice::NotInstalled.note().unwrap().contains("--path"));
    let note = Choice::Unverified.note().unwrap();
    assert!(
        note.contains("hash") && note.contains("normal way"),
        "{note}"
    );
    assert_eq!(Choice::On.note(), None);
    assert_eq!(Choice::Off.note(), None);
    let note = refused_fallback_note(730, "CS2", "VAC");
    assert!(
        note.contains("VAC") && note.contains("normal way"),
        "{note}"
    );
}

#[test]
fn old_configs_without_auto_still_load() {
    let old: LauncherConfig = serde_json::from_str(r#"{"steamworks_client": true}"#)
        .or_else(|_| toml_like_default())
        .unwrap();
    assert!(old.steamworks_client);
    assert!(!old.steamworks_client_auto);
}

/// Fallback when JSON lacks required keys.
fn toml_like_default() -> Result<LauncherConfig, serde_json::Error> {
    let mut value = serde_json::to_value(LauncherConfig::default())?;
    value["steamworks_client"] = serde_json::Value::Bool(true);
    value
        .as_object_mut()
        .unwrap()
        .remove("steamworks_client_auto");
    serde_json::from_value(value)
}

#[cfg(target_os = "linux")]
#[test]
fn installed_checks_the_recorded_hash() {
    use crate::launch::stand_in_lib::{hash_file, sha256_hex};
    let dir = tempfile::tempdir().unwrap();
    let lib = dir.path().join("sdk64/steamclient.so");
    let cfg = LauncherConfig {
        steamworks_client_path: Some(dir.path().to_string_lossy().into_owned()),
        ..LauncherConfig::default()
    };
    assert_eq!(installed(&cfg), Library::Missing);
    std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
    std::fs::write(&lib, b"lib").unwrap();
    assert_eq!(installed(&cfg), Library::Unverified, "no hash yet");
    std::fs::write(hash_file(&lib), sha256_hex(&lib).unwrap()).unwrap();
    assert_eq!(installed(&cfg), Library::Ready);
    std::fs::write(&lib, b"edited").unwrap();
    assert_eq!(installed(&cfg), Library::Unverified, "changed");
}
