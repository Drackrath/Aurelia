//! `config steam-runtime-runner` — the setter added so a first-time user has a clear,
//! CLI-driven way to configure `steam_runtime_runner` (instead of hand-editing JSON and
//! guessing what value is valid).
//!
//! Drives the real binary as a subprocess: the setting is persisted to `config.json`
//! under `AURELIA_CONFIG_DIR`, and `AURELIA_NO_DAEMON` keeps the command from forwarding
//! to a session daemon that would resolve config against its own environment.

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

/// A config dir whose library contains an installed GE-Proton tree (proton script +
/// bundled bare wine), so a set runner resolves.
fn sandbox_with_proton() -> TempDir {
    let tmp = tempdir().unwrap();
    let lib = tmp.path().join("lib");
    let ge = lib.join("compatibilitytools.d/GE-Proton9-20");
    fs::create_dir_all(ge.join("files/bin")).unwrap();
    fs::write(ge.join("proton"), "#!/usr/bin/env python3\n").unwrap();
    fs::write(ge.join("files/bin/wine64"), "#!/bin/sh\n").unwrap();
    fs::write(
        tmp.path().join("config.json"),
        format!(
            r#"{{ "steam_library_path": "{}", "proton_version": "x", "enable_cloud_sync": false }}"#,
            lib.display()
        ),
    )
    .unwrap();
    tmp
}

fn out(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn setting_an_installed_runner_persists_and_reports_resolved_wine() {
    let tmp = sandbox_with_proton();
    let o = aurelia(tmp.path(), &["config", "steam-runtime-runner", "GE-Proton9-20"]);
    assert!(o.status.success(), "{}", out(&o));

    let text = out(&o);
    assert!(text.contains("Resolves to bare Wine"), "{text}");
    assert!(text.contains("files/bin/wine64"), "{text}");

    // Persisted to config.json.
    let cfg = fs::read_to_string(tmp.path().join("config.json")).unwrap();
    assert!(cfg.contains("\"steam_runtime_runner\""), "{cfg}");
    assert!(cfg.contains("GE-Proton9-20"), "{cfg}");
}

#[test]
fn setting_an_uninstalled_runner_saves_but_warns_cleanly() {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("config.json"),
        r#"{ "steam_library_path": "/nonexistent", "proton_version": "x", "enable_cloud_sync": false }"#,
    )
    .unwrap();

    // A name that can't exist anywhere — resolve_runner also searches the machine's real
    // Steam compatibilitytools.d, so a real GE-Proton on the dev box would resolve here.
    let missing = "aurelia-nonexistent-runner-9x7";
    let o = aurelia(tmp.path(), &["config", "steam-runtime-runner", missing]);
    assert!(o.status.success(), "{}", out(&o));
    let text = out(&o);
    assert!(text.contains("does not resolve"), "{text}");
    assert!(text.contains("Saved."), "{text}");
    // The quiet resolver must NOT leak resolve_runner's tracing warning into output.
    assert!(
        !text.contains("returning the name verbatim"),
        "stray resolver warning leaked: {text}"
    );
}

#[test]
fn viewing_when_unset_explains_how_to_set_it() {
    let tmp = tempdir().unwrap();
    let o = aurelia(tmp.path(), &["config", "steam-runtime-runner"]);
    assert!(o.status.success(), "{}", out(&o));
    let text = out(&o);
    assert!(text.contains("(unset)"), "{text}");
    assert!(text.contains("aurelia proton list"), "{text}");
    assert!(text.contains("config steam-runtime-runner"), "{text}");
}

#[test]
fn empty_value_clears_the_runner() {
    let tmp = sandbox_with_proton();
    assert!(aurelia(tmp.path(), &["config", "steam-runtime-runner", "GE-Proton9-20"])
        .status
        .success());
    let o = aurelia(tmp.path(), &["config", "steam-runtime-runner", ""]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(out(&o).contains("(unset)"), "{}", out(&o));
}

#[test]
fn install_without_a_runner_gives_actionable_guidance() {
    let tmp = tempdir().unwrap();
    let o = aurelia(tmp.path(), &["steam-runtime", "install"]);
    assert!(!o.status.success());
    let text = out(&o);
    assert!(text.contains("aurelia proton list"), "{text}");
    assert!(text.contains("config steam-runtime-runner"), "{text}");
}
