//! `aurelia login` must warn about an already-active session.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::tempdir;

// No TTY here: login fails at the password
// prompt, before any Steam contact.
fn login(config_dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aurelia"))
        .arg("login")
        .env("AURELIA_CONFIG_DIR", config_dir)
        .env("AURELIA_NO_DAEMON", "1")
        .env("AURELIA_DISABLE_KEYRING", "1")
        .env_remove("AURELIA_PASSWORD")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("failed running the aurelia binary")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn plaintext_session_prefers_the_persona_name() {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("session.json"),
        r#"{"account_name":"tester_acct","persona_name":"Tester","refresh_token":"tok"}"#,
    )
    .unwrap();
    let err = stderr(&login(tmp.path()));
    assert!(
        err.contains("already logged in as Tester"),
        "missing warning: {err}"
    );
}

// Older sessions carry no persona name.
#[test]
fn plaintext_session_falls_back_to_the_account_name() {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("session.json"),
        r#"{"account_name":"tester_acct","refresh_token":"tok"}"#,
    )
    .unwrap();
    let err = stderr(&login(tmp.path()));
    assert!(
        err.contains("already logged in as tester_acct"),
        "missing warning: {err}"
    );
}

#[test]
fn locked_session_with_a_name_hint_warns_with_the_name() {
    let tmp = tempdir().unwrap();
    let mut envelope = aurelia::core::session_crypto::encrypt(
        br#"{"account_name":"tester_acct","refresh_token":"tok"}"#,
        "hunter2",
    )
    .unwrap();
    // As save_session writes it.
    envelope.display_name = Some("Tester".to_string());
    fs::write(
        tmp.path().join("session.json"),
        serde_json::to_string(&envelope).unwrap(),
    )
    .unwrap();
    let err = stderr(&login(tmp.path()));
    assert!(
        err.contains("already logged in as Tester"),
        "missing warning: {err}"
    );
    assert!(
        !err.contains("Session password:"),
        "the warning must never prompt: {err}"
    );
}

// Pre-hint envelopes lack the name.
#[test]
fn locked_hintless_session_warns_without_a_name() {
    let tmp = tempdir().unwrap();
    let envelope = aurelia::core::session_crypto::encrypt(
        br#"{"account_name":"tester_acct","refresh_token":"tok"}"#,
        "hunter2",
    )
    .unwrap();
    fs::write(
        tmp.path().join("session.json"),
        serde_json::to_string(&envelope).unwrap(),
    )
    .unwrap();
    let err = stderr(&login(tmp.path()));
    assert!(
        err.contains("an existing session is active"),
        "missing warning: {err}"
    );
}

#[test]
fn no_session_prints_no_warning() {
    let tmp = tempdir().unwrap();
    let err = stderr(&login(tmp.path()));
    assert!(!err.contains("Warning:"), "unexpected warning: {err}");
}

#[test]
fn empty_session_prints_no_warning() {
    let tmp = tempdir().unwrap();
    fs::write(tmp.path().join("session.json"), "{}").unwrap();
    let err = stderr(&login(tmp.path()));
    assert!(!err.contains("Warning:"), "unexpected warning: {err}");
}
