//! Encrypted-session unlock and error-surfacing regression tests.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::{tempdir, TempDir};

const PASSWORD: &str = "hunter2";

fn aurelia(config_dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aurelia"));
    cmd.args(args)
        .env("AURELIA_CONFIG_DIR", config_dir)
        .env("AURELIA_NO_DAEMON", "1")
        .env_remove("AURELIA_SESSION_PASSWORD")
        .stdin(std::process::Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("failed running the aurelia binary")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

// Tokenless session: restore fails before networking.
fn with_encrypted_session() -> TempDir {
    let tmp = tempdir().unwrap();
    let envelope = aurelia::core::session_crypto::encrypt(b"{}", PASSWORD).unwrap();
    fs::write(
        tmp.path().join("session.json"),
        serde_json::to_string(&envelope).unwrap(),
    )
    .unwrap();
    tmp
}

#[test]
fn missing_password_is_not_reported_as_logged_out() {
    let tmp = with_encrypted_session();
    let out = aurelia(tmp.path(), &["account"], &[]);
    assert!(!out.status.success(), "expected failure, got: {out:?}");
    let err = stderr(&out);
    assert!(
        err.contains("daemon unlock") || err.contains("AURELIA_SESSION_PASSWORD"),
        "error must point at the unlock paths: {err}"
    );
    assert!(
        !err.contains("not logged in"),
        "must not misreport as logged out: {err}"
    );
}

#[test]
fn wrong_password_is_surfaced() {
    let tmp = with_encrypted_session();
    let out = aurelia(
        tmp.path(),
        &["account"],
        &[("AURELIA_SESSION_PASSWORD", "wrong")],
    );
    assert!(!out.status.success(), "expected failure, got: {out:?}");
    let err = stderr(&out);
    assert!(
        err.contains("wrong session password"),
        "error must name the bad password: {err}"
    );
    assert!(
        !err.contains("not logged in"),
        "must not misreport as logged out: {err}"
    );
}

#[test]
fn unlock_without_a_session_refuses() {
    let tmp = tempdir().unwrap();
    let out = aurelia(tmp.path(), &["daemon", "unlock"], &[]);
    assert!(!out.status.success(), "expected failure, got: {out:?}");
    assert!(
        stderr(&out).contains("no stored session"),
        "unexpected error: {}",
        stderr(&out)
    );
}

#[test]
fn unlock_on_a_plaintext_session_is_a_no_op() {
    let tmp = tempdir().unwrap();
    fs::write(tmp.path().join("session.json"), "{}").unwrap();
    let out = aurelia(
        tmp.path(),
        &["daemon", "unlock"],
        &[("AURELIA_SESSION_PASSWORD", PASSWORD)],
    );
    assert!(out.status.success(), "expected success, got: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("not encrypted"),
        "unexpected output: {stdout}"
    );
}

#[cfg(unix)]
mod daemon_e2e {
    use super::*;
    use std::time::{Duration, Instant};

    // Daemon subprocess; killed on drop.
    struct DaemonProc(std::process::Child);

    impl Drop for DaemonProc {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn spawn_daemon(config_dir: &Path, socket: &Path) -> DaemonProc {
        let child = Command::new(env!("CARGO_BIN_EXE_aurelia"))
            .args(["daemon"])
            .env("AURELIA_CONFIG_DIR", config_dir)
            .env("AURELIA_DAEMON_SOCKET", socket)
            .env_remove("AURELIA_SESSION_PASSWORD")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("failed spawning the daemon");
        // Marker too, so clients skip mismatch handling.
        let marker = socket.with_extension("info");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !socket.exists() || !marker.exists() {
            assert!(Instant::now() < deadline, "daemon socket never appeared");
            std::thread::sleep(Duration::from_millis(50));
        }
        DaemonProc(child)
    }

    fn forwarded(config_dir: &Path, socket: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aurelia"));
        cmd.args(args)
            .env("AURELIA_CONFIG_DIR", config_dir)
            .env("AURELIA_DAEMON_SOCKET", socket)
            .env("AURELIA_NO_SPAWN", "1")
            .env_remove("AURELIA_SESSION_PASSWORD")
            .env_remove("AURELIA_NO_DAEMON")
            .stdin(std::process::Stdio::null());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.output().expect("failed running the aurelia binary")
    }

    #[test]
    fn daemon_accepts_the_password_and_reports_restore_errors() {
        let tmp = with_encrypted_session();
        let socket = tmp.path().join("daemon.sock");
        let _daemon = spawn_daemon(tmp.path(), &socket);

        // Locked daemon: clear error, not "not logged in".
        let out = forwarded(tmp.path(), &socket, &["account"], &[]);
        assert!(!out.status.success(), "expected failure, got: {out:?}");
        let err = stderr(&out);
        assert!(
            err.contains("could not restore the stored session"),
            "unexpected error: {err}"
        );
        assert!(
            !err.contains("not logged in"),
            "must not misreport as logged out: {err}"
        );

        // Wrong password: rejected client-side.
        let out = forwarded(
            tmp.path(),
            &socket,
            &["daemon", "unlock"],
            &[("AURELIA_SESSION_PASSWORD", "wrong")],
        );
        assert!(!out.status.success(), "expected failure, got: {out:?}");
        assert!(
            stderr(&out).contains("wrong session password"),
            "unexpected error: {}",
            stderr(&out)
        );

        // Right password: accepted; tokenless restore warns.
        let out = forwarded(
            tmp.path(),
            &socket,
            &["daemon", "unlock"],
            &[("AURELIA_SESSION_PASSWORD", PASSWORD)],
        );
        assert!(out.status.success(), "expected success, got: {out:?}");
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("Daemon unlocked."),
            "unexpected output: {out:?}"
        );

        // Unlocked: failure is the token, not the password.
        let out = forwarded(tmp.path(), &socket, &["account"], &[]);
        assert!(!out.status.success(), "expected failure, got: {out:?}");
        let err = stderr(&out);
        assert!(
            err.contains("no persisted account_name found"),
            "expected a decrypted-session restore error: {err}"
        );
    }
}
