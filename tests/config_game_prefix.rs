//! `config game <appid> --prefix DIR` and `--no-prefix`.

mod common;
use common::{aurelia, out};

use std::fs;
use tempfile::{TempDir, tempdir};

fn sandbox() -> TempDir {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("config.json"),
        r#"{ "steam_library_path": "/x", "proton_version": "x", "enable_cloud_sync": false }"#,
    )
    .unwrap();
    tmp
}

fn game_json(tmp: &TempDir, extra: &[&str]) -> serde_json::Value {
    let mut args = vec!["--json", "config", "game", "570"];
    args.extend_from_slice(extra);
    let o = aurelia(tmp.path(), &args);
    assert!(o.status.success(), "{}", out(&o));
    serde_json::from_slice(&o.stdout).unwrap()
}

#[test]
fn prefix_sets_and_clears_the_game_prefix() {
    let tmp = sandbox();
    let dir = tmp.path().join("Prefixes/Dota");
    let v = game_json(&tmp, &["--prefix", dir.to_str().unwrap()]);
    assert_eq!(v["prefix_dir"], dir.to_str().unwrap());
    assert_eq!(v["wine_prefix"], dir.join("pfx").to_str().unwrap());

    let v = game_json(&tmp, &["--no-prefix"]);
    assert!(v["prefix_dir"].is_null(), "{v}");
    assert_eq!(v["wine_prefix"], "/x/steamapps/compatdata/570/pfx");
}

#[test]
fn prefix_conflicts_with_no_prefix() {
    let tmp = sandbox();
    let o = aurelia(
        tmp.path(),
        &["config", "game", "570", "--prefix", "/p", "--no-prefix"],
    );
    assert!(!o.status.success(), "{}", out(&o));
}

#[test]
fn clearing_unset_values_writes_nothing() {
    let tmp = sandbox();
    let before = fs::read(tmp.path().join("config.json")).unwrap();
    game_json(&tmp, &["--no-prefix", "--no-launch-options"]);
    assert_eq!(fs::read(tmp.path().join("config.json")).unwrap(), before);
    assert!(!tmp.path().join("user_apps.json").exists());
}

#[test]
fn set_then_clear_leaves_no_entry() {
    let tmp = sandbox();
    game_json(&tmp, &["--prefix", "/p"]);
    game_json(&tmp, &["--no-prefix"]);
    let cfg: serde_json::Value =
        serde_json::from_slice(&fs::read(tmp.path().join("config.json")).unwrap()).unwrap();
    assert!(cfg["game_configs"].get("570").is_none(), "{cfg}");
}
