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
        let got = decide(&config(on, auto), || running, || installed);
        assert_eq!(
            got, want,
            "on={on} auto={auto} running={running} installed={installed}"
        );
    }
}

#[test]
fn detectors_run_only_in_auto() {
    let never = || -> bool { panic!("detector ran") };
    assert_eq!(decide(&config(false, false), never, never), Choice::Off);
    assert_eq!(decide(&config(true, false), never, never), Choice::On);
    // Steam running: no install check.
    assert_eq!(
        decide(&config(false, true), || true, never),
        Choice::SteamRunning
    );
}

#[test]
fn apply_sets_the_launch_flag_only() {
    let mut cfg = config(false, true);
    assert_eq!(apply(&mut cfg, || false, |_| true), Choice::StandIn);
    assert!(cfg.steamworks_client && cfg.steamworks_client_auto);
    let mut cfg = config(false, true);
    assert_eq!(apply(&mut cfg, || true, |_| true), Choice::SteamRunning);
    assert!(!cfg.steamworks_client);
    let mut cfg = config(true, false);
    assert_eq!(apply(&mut cfg, || true, |_| false), Choice::On);
    assert!(cfg.steamworks_client);
}

#[test]
fn notes_name_the_path_and_reason() {
    assert!(Choice::StandIn.note().unwrap().starts_with("stand-in:"));
    assert!(Choice::SteamRunning.note().unwrap().contains("running"));
    assert!(Choice::NotInstalled.note().unwrap().contains("--path"));
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
