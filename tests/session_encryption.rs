//! Encrypted-session error-surfacing regression tests.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::{tempdir, TempDir};

fn aurelia(config_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aurelia"))
        .args(args)
        .env("AURELIA_CONFIG_DIR", config_dir)
        .env("AURELIA_NO_DAEMON", "1")
        .env("AURELIA_DISABLE_KEYRING", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("failed running the aurelia binary")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

// Tokenless session: restore fails before networking.
fn write_envelope(dir: &Path, keyring: bool) {
    let mut envelope = aurelia::core::session_crypto::encrypt(b"{}", "irrelevant").unwrap();
    envelope.keyring = keyring;
    fs::write(
        dir.join("session.json"),
        serde_json::to_string(&envelope).unwrap(),
    )
    .unwrap();
}

fn with_keyring_session() -> TempDir {
    let tmp = tempdir().unwrap();
    write_envelope(tmp.path(), true);
    tmp
}

#[test]
fn keyring_mode_without_a_keyring_names_the_keyring() {
    let tmp = with_keyring_session();
    let out = aurelia(tmp.path(), &["account"]);
    assert!(!out.status.success(), "expected failure, got: {out:?}");
    let err = stderr(&out);
    assert!(
        err.contains("OS keyring"),
        "error must name the keyring: {err}"
    );
    assert!(
        !err.contains("not logged in"),
        "must not misreport as logged out: {err}"
    );
}

#[test]
fn legacy_password_envelope_points_at_relogin() {
    let tmp = tempdir().unwrap();
    write_envelope(tmp.path(), false);
    let out = aurelia(tmp.path(), &["account"]);
    assert!(!out.status.success(), "expected failure, got: {out:?}");
    let err = stderr(&out);
    assert!(
        err.contains("legacy session password") && err.contains("aurelia login"),
        "error must point at a re-login: {err}"
    );
    assert!(
        !err.contains("not logged in"),
        "must not misreport as logged out: {err}"
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
            .env("AURELIA_DISABLE_KEYRING", "1")
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

    #[test]
    fn logged_out_stays_a_plain_not_logged_in() {
        let tmp = tempdir().unwrap();
        let socket = tmp.path().join("daemon.sock");
        let _daemon = spawn_daemon(tmp.path(), &socket);

        let out = Command::new(env!("CARGO_BIN_EXE_aurelia"))
            .args(["account"])
            .env("AURELIA_CONFIG_DIR", tmp.path())
            .env("AURELIA_DAEMON_SOCKET", &socket)
            .env("AURELIA_DISABLE_KEYRING", "1")
            .env("AURELIA_NO_SPAWN", "1")
            .env_remove("AURELIA_NO_DAEMON")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("failed running the aurelia binary");

        assert!(!out.status.success(), "expected failure, got: {out:?}");
        assert!(
            stderr(&out).contains("not logged in"),
            "a missing session is simply logged out: {}",
            stderr(&out)
        );
    }

    #[test]
    fn daemon_surfaces_the_restore_error() {
        let tmp = with_keyring_session();
        let socket = tmp.path().join("daemon.sock");
        let _daemon = spawn_daemon(tmp.path(), &socket);

        let out = Command::new(env!("CARGO_BIN_EXE_aurelia"))
            .args(["account"])
            .env("AURELIA_CONFIG_DIR", tmp.path())
            .env("AURELIA_DAEMON_SOCKET", &socket)
            .env("AURELIA_DISABLE_KEYRING", "1")
            .env("AURELIA_NO_SPAWN", "1")
            .env_remove("AURELIA_NO_DAEMON")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("failed running the aurelia binary");

        assert!(!out.status.success(), "expected failure, got: {out:?}");
        let err = stderr(&out);
        assert!(
            err.contains("could not restore the stored session") && err.contains("OS keyring"),
            "unexpected error: {err}"
        );
        assert!(
            !err.contains("not logged in"),
            "must not misreport as logged out: {err}"
        );
    }
}
