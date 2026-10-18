//! `aurelia logout` always clears the local session.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::tempdir;

// No refresh token, no Steam contact.
fn logout(config_dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aurelia"))
        .args(["logout", "--json"])
        .env("AURELIA_CONFIG_DIR", config_dir)
        .env("AURELIA_NO_DAEMON", "1")
        .env("AURELIA_DISABLE_KEYRING", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("failed running the aurelia binary")
}

fn json(out: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("bad json ({e}): {stdout}"))
}

#[test]
fn session_without_token_is_removed_unrevoked() {
    let tmp = tempdir().unwrap();
    let session = tmp.path().join("session.json");
    fs::write(&session, r#"{"account_name":"tester_acct"}"#).unwrap();

    let out = logout(tmp.path());
    assert!(out.status.success(), "logout failed: {}", String::from_utf8_lossy(&out.stderr));
    let body = json(&out);
    assert_eq!(body["logged_out"], true);
    assert_eq!(body["token_revoked"], false);
    assert!(body["warning"].is_string());
    assert!(!session.exists(), "session.json survived logout");
}

#[test]
fn unreadable_session_is_still_removed() {
    let tmp = tempdir().unwrap();
    let session = tmp.path().join("session.json");
    fs::write(&session, "{ not json").unwrap();

    let out = logout(tmp.path());
    assert!(out.status.success(), "logout failed: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(json(&out)["token_revoked"], false);
    assert!(!session.exists(), "session.json survived logout");
}
