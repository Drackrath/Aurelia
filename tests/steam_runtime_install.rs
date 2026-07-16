//! Regression tests for issue #2: `SteamSetup.exe` downloaded into `runtimes/` but
//! never executed, leaving `steam.exe present : no` forever.
//!
//! The reporter had `steam_runtime_runner = GE-Proton9-20`, so the installer was
//! driven through `proton run` — a wrapper that ignores the `WINEPREFIX` the caller
//! sets and expects the Steam Linux Runtime container.

use aurelia::core::utils::{
    build_runner_command, resolve_runner_opt, resolve_steam_runtime_wine,
    steam_runtime_runner_unset_msg,
};
use aurelia::launch::is_valid_setup_exe;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

/// Build a Proton tree shaped like GE-Proton9-20: a `proton` launch script at the
/// root plus a bundled bare wine under `files/bin/`.
fn fake_proton_tree(root: &Path, name: &str) -> PathBuf {
    let dir = root.join("compatibilitytools.d").join(name);
    fs::create_dir_all(dir.join("files/bin")).unwrap();
    fs::write(dir.join("proton"), "#!/usr/bin/env python3\n").unwrap();
    fs::write(dir.join("files/bin/wine64"), "#!/bin/sh\n").unwrap();
    dir
}

/// Build a bare wine tree (wine-tkg / plain Wine layout).
fn fake_wine_tree(root: &Path, name: &str) -> PathBuf {
    let dir = root.join("compatibilitytools.d").join(name);
    fs::create_dir_all(dir.join("bin")).unwrap();
    fs::write(dir.join("bin/wine64"), "#!/bin/sh\n").unwrap();
    dir
}

fn library() -> (TempDir, PathBuf) {
    let tmp = tempdir().unwrap();
    let lib = tmp.path().join("lib");
    fs::create_dir_all(&lib).unwrap();
    (tmp, lib)
}

#[test]
fn proton_runner_resolves_to_bundled_bare_wine_not_proton_run() {
    let (_tmp, lib) = library();
    let proton = fake_proton_tree(&lib, "GE-Proton9-20");

    let wine = resolve_steam_runtime_wine("GE-Proton9-20", &lib).unwrap();

    // The regression: this used to yield `proton run`.
    assert_eq!(wine, proton.join("files/bin/wine64"));
    assert_ne!(wine.file_name().unwrap(), "proton");
}

/// Pins the exact behavior that caused issue #2, so the two helpers can't silently
/// converge again: `build_runner_command` is right for games, wrong for Steam.
#[test]
fn build_runner_command_still_yields_proton_run_for_games() {
    let (_tmp, lib) = library();
    let proton = fake_proton_tree(&lib, "GE-Proton9-20");

    let cmd = build_runner_command(&proton).unwrap();
    assert_eq!(Path::new(cmd.get_program()), proton.join("proton"));
    let args: Vec<_> = cmd.get_args().collect();
    assert_eq!(args, ["run"]);
}

#[test]
fn bare_wine_tree_resolves_to_its_wine_binary() {
    let (_tmp, lib) = library();
    let wine_tree = fake_wine_tree(&lib, "wine-tkg-9.0");

    let wine = resolve_steam_runtime_wine("wine-tkg-9.0", &lib).unwrap();
    assert_eq!(wine, wine_tree.join("bin/wine64"));
}

#[test]
fn absolute_path_to_wine_binary_is_accepted() {
    let (_tmp, lib) = library();
    let wine_tree = fake_wine_tree(&lib, "wine-tkg-9.0");
    let direct = wine_tree.join("bin/wine64");

    let wine = resolve_steam_runtime_wine(direct.to_str().unwrap(), &lib).unwrap();
    assert_eq!(wine, direct);
}

#[test]
fn proton_script_passed_directly_still_unwraps_to_bare_wine() {
    let (_tmp, lib) = library();
    let proton = fake_proton_tree(&lib, "GE-Proton9-20");
    let script = proton.join("proton");

    let wine = resolve_steam_runtime_wine(script.to_str().unwrap(), &lib).unwrap();
    assert_eq!(wine, proton.join("files/bin/wine64"));
}

#[test]
fn empty_runner_name_is_rejected() {
    let (_tmp, lib) = library();
    let err = resolve_steam_runtime_wine("", &lib).unwrap_err().to_string();
    assert!(
        err.contains("No Steam Runtime Runner selected"),
        "unexpected error: {err}"
    );
}

#[test]
fn unknown_runner_name_reports_what_it_looked_for() {
    let (_tmp, lib) = library();
    let err = resolve_steam_runtime_wine("GE-Proton9-20", &lib)
        .unwrap_err()
        .to_string();
    assert!(err.contains("could not be found"), "unexpected error: {err}");
    assert!(err.contains("GE-Proton9-20"), "unexpected error: {err}");
}

#[test]
fn proton_tree_without_bundled_wine_fails_with_actionable_error() {
    let (_tmp, lib) = library();
    // A Proton tree with the launch script but no bundled wine.
    let dir = lib.join("compatibilitytools.d/GE-Proton-Broken");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("proton"), "#!/usr/bin/env python3\n").unwrap();

    let err = resolve_steam_runtime_wine("GE-Proton-Broken", &lib)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("no bundled wine binary"),
        "unexpected error: {err}"
    );
}

#[test]
fn runner_dir_that_is_not_a_wine_tree_is_rejected() {
    let (_tmp, lib) = library();
    let dir = lib.join("compatibilitytools.d/NotARunner");
    fs::create_dir_all(dir.join("share")).unwrap();

    let err = resolve_steam_runtime_wine("NotARunner", &lib)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("not a usable wine runner"),
        "unexpected error: {err}"
    );
}

// --- cached SteamSetup.exe validation ---------------------------------------
//
// The download used to be guarded by `exists()` alone, so a CDN error page or a
// download interrupted midway was cached as SteamSetup.exe and handed to wine on
// every later install — the "it installs in the runtimes directory and that's it"
// half of issue #2.

fn setup_exe_containing(bytes: &[u8]) -> (TempDir, PathBuf) {
    let tmp = tempdir().unwrap();
    let exe = tmp.path().join("SteamSetup.exe");
    fs::write(&exe, bytes).unwrap();
    (tmp, exe)
}

#[test]
fn cached_pe_executable_is_accepted() {
    let (_tmp, exe) = setup_exe_containing(b"MZ\x90\x00\x03");
    assert!(is_valid_setup_exe(&exe));
}

#[test]
fn cached_html_error_page_is_rejected() {
    let (_tmp, exe) = setup_exe_containing(b"<!DOCTYPE html><html>403 Forbidden</html>");
    assert!(!is_valid_setup_exe(&exe));
}

#[test]
fn cached_truncated_download_is_rejected() {
    let (_tmp, exe) = setup_exe_containing(b"M");
    assert!(!is_valid_setup_exe(&exe));
}

#[test]
fn cached_empty_file_is_rejected() {
    let (_tmp, exe) = setup_exe_containing(b"");
    assert!(!is_valid_setup_exe(&exe));
}

#[test]
fn missing_setup_exe_is_rejected() {
    let tmp = tempdir().unwrap();
    assert!(!is_valid_setup_exe(&tmp.path().join("nope.exe")));
}

// --- runner selection UX (issue: "not clear what to set the runner to") -----
//
// A first-time user hit `install` with no runner set and had no idea what value was
// valid. The guidance must name the discovery command and the setter, and resolution
// used by the config setter must be quiet (no stray warning log) so the setter can
// probe validity cleanly.

#[test]
fn unset_message_points_to_discovery_and_setter() {
    let msg = steam_runtime_runner_unset_msg("installing");
    assert!(msg.contains("installing"), "{msg}");
    assert!(msg.contains("aurelia proton list"), "{msg}");
    assert!(msg.contains("aurelia config steam-runtime-runner"), "{msg}");
}

#[test]
fn quiet_resolver_finds_installed_runner() {
    let (_tmp, lib) = library();
    let proton = fake_proton_tree(&lib, "GE-Proton9-20");
    // Same result as resolve_runner, but returns Option so callers can probe silently.
    assert_eq!(resolve_runner_opt("GE-Proton9-20", &lib), Some(proton));
}

#[test]
fn quiet_resolver_returns_none_for_unknown_runner() {
    let (_tmp, lib) = library();
    assert_eq!(resolve_runner_opt("GE-Proton9-20", &lib), None);
}

/// The quiet resolver matches fuzzily just like resolve_runner, so a config-setter probe
/// won't false-negative a valid runtime the user typed loosely.
#[test]
fn quiet_resolver_matches_fuzzily() {
    let (_tmp, lib) = library();
    let dir = lib.join("steamapps/common/Proton - Experimental");
    fs::create_dir_all(dir.join("files/bin")).unwrap();
    fs::write(dir.join("proton"), "#!/usr/bin/env python3\n").unwrap();
    fs::write(dir.join("files/bin/wine64"), "#!/bin/sh\n").unwrap();

    assert_eq!(resolve_runner_opt("experimental", &lib), Some(dir));
}
