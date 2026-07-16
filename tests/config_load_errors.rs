//! A corrupt `config.json` must never be silently replaced by defaults.
//!
//! `load_launcher_config` already returns `Ok(defaults)` when the file is *missing*, so
//! an `Err` only ever means "the file exists but could not be parsed". Callers used to
//! paper over that with `.unwrap_or_default()`, which meant a single typo made every
//! setting vanish — and any command that then saved wrote those defaults straight over
//! the user's file.
//!
//! These drive the real binary as a subprocess: config resolution reads the
//! `AURELIA_CONFIG_DIR` env var, and per-process isolation keeps parallel tests from
//! racing on it. `AURELIA_NO_DAEMON` stops the CLI from forwarding to a session daemon
//! (which would resolve the config against the *daemon's* environment, not ours).

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::{tempdir, TempDir};

const CORRUPT: &str = r#"{
  "steam_library_path": "/mnt/games/SteamLibrary",
  "proton_version": "GE-Proton9-20",
  "enable_cloud_sync": tru,
  "steam_runtime_runner": "wine-tkg-9.0",
  "preferred_launch_options": { "570": "-novid -high" },
  "language": "german"
}"#;

fn aurelia(config_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aurelia"))
        .args(args)
        .env("AURELIA_CONFIG_DIR", config_dir)
        .env("AURELIA_NO_DAEMON", "1")
        .output()
        .expect("failed running the aurelia binary")
}

fn with_corrupt_config() -> TempDir {
    let tmp = tempdir().unwrap();
    fs::write(tmp.path().join("config.json"), CORRUPT).unwrap();
    tmp
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// The headline regression: `config language` mutates and saves. With a corrupt config
/// it used to write defaults over the file, wiping every unrelated setting.
#[test]
fn corrupt_config_is_not_overwritten_by_a_saving_command() {
    let tmp = with_corrupt_config();
    let path = tmp.path().join("config.json");

    let out = aurelia(tmp.path(), &["config", "language", "english"]);

    assert!(!out.status.success(), "expected failure, got: {out:?}");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        CORRUPT,
        "the user's config.json was modified"
    );
}

/// The papercut from issue #2's review: an unparseable config surfaced as
/// "no Steam Runtime Runner is configured" instead of a parse error.
#[test]
fn corrupt_config_reports_a_parse_error_not_a_missing_runner() {
    let tmp = with_corrupt_config();
    let err = stderr(&aurelia(tmp.path(), &["steam-runtime", "install"]));

    assert!(err.contains("invalid launcher config"), "got: {err}");
    assert!(
        !err.contains("no Steam Runtime Runner is configured"),
        "corruption was misreported as an unset runner: {err}"
    );
}

#[test]
fn corrupt_config_error_names_the_file_and_location() {
    let tmp = with_corrupt_config();
    let err = stderr(&aurelia(tmp.path(), &["config", "show"]));

    assert!(err.contains("config.json"), "got: {err}");
    assert!(err.contains("line 4"), "expected a location: {err}");
    assert!(
        err.contains("delete it to regenerate defaults"),
        "expected a suggested fix: {err}"
    );
}

#[test]
fn corrupt_config_fails_every_reading_command() {
    let tmp = with_corrupt_config();
    for args in [
        vec!["config", "show"],
        vec!["steam-runtime", "status"],
        vec!["steam-runtime", "install"],
        vec!["proton", "list"],
    ] {
        let out = aurelia(tmp.path(), &args);
        assert!(
            !out.status.success(),
            "`{}` silently accepted a corrupt config",
            args.join(" ")
        );
    }
}

/// The other half of the contract: a *missing* config is the normal first-run path and
/// must keep yielding defaults. Propagating errors must not break new users.
#[test]
fn missing_config_still_yields_defaults() {
    let tmp = tempdir().unwrap();
    let out = aurelia(tmp.path(), &["config", "show"]);

    assert!(out.status.success(), "fresh install failed: {}", stderr(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("steam_library_path"), "got: {stdout}");
}

#[test]
fn missing_config_can_still_be_written() {
    let tmp = tempdir().unwrap();
    let out = aurelia(tmp.path(), &["config", "language", "english"]);
    assert!(out.status.success(), "failed: {}", stderr(&out));

    let written = fs::read_to_string(tmp.path().join("config.json")).unwrap();
    assert!(written.contains("\"language\": \"english\""), "got: {written}");
}
