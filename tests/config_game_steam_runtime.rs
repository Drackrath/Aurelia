//! `config game <appid> --steam-runtime <auto|on|off>` — the CLI setter for the per-game
//! Steam-runtime policy that gates the self-contained Windows Steam runtime at launch.
//!
//! Before this, the only way to enable it was hand-editing `user_apps.json` (every
//! non-defaulted field required, and a typo silently dropped all per-game config at
//! launch). These drive the real binary with an isolated config dir; `AURELIA_NO_DAEMON`
//! keeps the command from forwarding to a session daemon.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::{tempdir, TempDir};

fn aurelia(config_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aurelia"))
        .args(args)
        .env("AURELIA_CONFIG_DIR", config_dir)
        .env("AURELIA_NO_DAEMON", "1")
        .output()
        .expect("failed running the aurelia binary")
}

fn out(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn sandbox() -> TempDir {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("config.json"),
        r#"{ "steam_library_path": "/x", "proton_version": "x", "enable_cloud_sync": false }"#,
    )
    .unwrap();
    tmp
}

#[test]
fn enabling_writes_a_complete_valid_user_apps_entry() {
    let tmp = sandbox();
    let o = aurelia(tmp.path(), &["config", "game", "570", "--steam-runtime", "on"]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(out(&o).contains("Steam runtime: on"), "{}", out(&o));

    let json = fs::read_to_string(tmp.path().join("user_apps.json")).unwrap();
    // The whole point: the setter writes a full entry, not a partial one that would fail
    // to parse. Round-tripping it back through the CLI must succeed.
    assert!(json.contains("\"steam_runtime_policy\": \"Enabled\""), "{json}");
    assert!(json.contains("\"launch_options\""), "{json}"); // a non-defaulted field is present
    let view = aurelia(tmp.path(), &["config", "game", "570"]);
    assert!(view.status.success(), "{}", out(&view));
    assert!(out(&view).contains("Steam runtime: on"), "{}", out(&view));
}

#[test]
fn prefix_mode_round_trips() {
    let tmp = sandbox();
    let o = aurelia(
        tmp.path(),
        &["config", "game", "570", "--steam-prefix-mode", "per-game"],
    );
    assert!(o.status.success(), "{}", out(&o));
    assert!(out(&o).contains("Prefix mode  : per-game"), "{}", out(&o));
}

#[test]
fn off_and_auto_are_distinct() {
    let tmp = sandbox();
    assert!(out(&aurelia(tmp.path(), &["config", "game", "570", "--steam-runtime", "off"]))
        .contains("Steam runtime: off"));
    assert!(out(&aurelia(tmp.path(), &["config", "game", "570", "--steam-runtime", "auto"]))
        .contains("Steam runtime: auto"));
}

/// The load-hardening half: a corrupt user_apps.json must fail loudly, not silently reset
/// every game's settings.
#[test]
fn corrupt_user_apps_errors_instead_of_resetting() {
    let tmp = sandbox();
    fs::write(
        tmp.path().join("user_apps.json"),
        r#"{ "570": { "steam_runtime_policy": Enabled } }"#,
    )
    .unwrap();

    let o = aurelia(tmp.path(), &["config", "game", "570"]);
    assert!(!o.status.success(), "should have failed: {}", out(&o));
    assert!(out(&o).contains("invalid per-game config"), "{}", out(&o));
}

/// The setter must not clobber a corrupt file — it loads first, so a bad file blocks the
/// write rather than overwriting (and losing) whatever the user was trying to fix.
#[test]
fn setter_refuses_to_overwrite_a_corrupt_file() {
    let tmp = sandbox();
    let corrupt = r#"{ "570": { bad json } }"#;
    fs::write(tmp.path().join("user_apps.json"), corrupt).unwrap();

    let o = aurelia(tmp.path(), &["config", "game", "570", "--steam-runtime", "on"]);
    assert!(!o.status.success(), "{}", out(&o));
    assert_eq!(
        fs::read_to_string(tmp.path().join("user_apps.json")).unwrap(),
        corrupt,
        "the corrupt file was overwritten"
    );
}
