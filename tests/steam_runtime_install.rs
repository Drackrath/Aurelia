//! Regression tests for issue #2: `SteamSetup.exe` downloaded into `runtimes/` but
//! never executed, leaving `steam.exe present : no` forever.
//!
//! The reporter had `steam_runtime_runner = GE-Proton9-20`, so the installer was
//! driven through `proton run` — a wrapper that ignores the `WINEPREFIX` the caller
//! sets and expects the Steam Linux Runtime container.

use aurelia::core::utils::{
    build_runner_command, find_steam_exe_in_prefix, resolve_runner_opt,
    resolve_steam_runtime_wine, steam_runtime_runner_unset_msg,
};
use aurelia::launch::{is_valid_setup_exe, preserve_steam_data_dirs, restore_steam_data_dirs};
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
    // A name that cannot exist on any real system — resolve_runner also searches the
    // machine's Steam compatibilitytools.d, so a plausible name like "GE-Proton9-20"
    // would resolve on a dev box that happens to have it installed.
    let missing = "aurelia-nonexistent-runner-9x7";
    let err = resolve_steam_runtime_wine(missing, &lib).unwrap_err().to_string();
    assert!(err.contains("could not be found"), "unexpected error: {err}");
    assert!(err.contains(missing), "unexpected error: {err}");
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

// --- steam.exe detection is case-insensitive -------------------------------
//
// The real Steam installer writes `Steam.exe` (capital S). On a case-sensitive Linux
// filesystem a hardcoded lowercase `steam.exe` misses it, so a *successful* install used
// to report "no steam.exe appeared". (The "and that's it" half of issue #2 once the
// installer itself was fixed to run silently.)

fn prefix_with_steam(leaf: &str) -> (TempDir, PathBuf) {
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("drive_c/Program Files (x86)/Steam");
    fs::create_dir_all(&dir).unwrap();
    let exe = dir.join(leaf);
    fs::write(&exe, b"MZ").unwrap();
    (tmp, exe)
}

#[test]
fn finds_capital_s_steam_exe() {
    let (tmp, exe) = prefix_with_steam("Steam.exe");
    assert_eq!(find_steam_exe_in_prefix(tmp.path()), Some(exe));
}

#[test]
fn finds_lowercase_steam_exe() {
    let (tmp, exe) = prefix_with_steam("steam.exe");
    assert_eq!(find_steam_exe_in_prefix(tmp.path()), Some(exe));
}

#[test]
fn no_steam_exe_returns_none() {
    let tmp = tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("drive_c/Program Files (x86)/Steam")).unwrap();
    assert_eq!(find_steam_exe_in_prefix(tmp.path()), None);
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
    // Bogus name so a system-installed GE-Proton on the dev box can't satisfy it.
    assert_eq!(resolve_runner_opt("aurelia-nonexistent-runner-9x7", &lib), None);
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

// --- data-preserving repair: preserve/restore round-trip ---------------------
//
// `repair` used to rename the whole prefix to `.bak` and reinstall, losing logins
// (`config`/`userdata`) AND games installed inside the in-Wine Steam (`steamapps`).
// The preserve/restore helpers move those dirs into a holding folder OUTSIDE the
// Steam dir until SteamSetup.exe exits (NSIS refuses a non-empty destination),
// then bring them back. Pure filesystem — no wine involved.

/// A fixture Steam dir: the three user-data dirs (with content) plus client files
/// the repair is allowed to lose.
fn fixture_steam_dir(root: &Path) -> PathBuf {
    let steam = root.join("drive_c/Program Files (x86)/Steam");
    fs::create_dir_all(steam.join("userdata/12345678/config")).unwrap();
    fs::write(steam.join("userdata/12345678/config/localconfig.vdf"), "\"UserLocalConfigStore\"{}").unwrap();
    fs::create_dir_all(steam.join("config")).unwrap();
    fs::write(steam.join("config/loginusers.vdf"), "\"users\"{}").unwrap();
    fs::create_dir_all(steam.join("steamapps/common/Some Game")).unwrap();
    fs::write(steam.join("steamapps/appmanifest_620.acf"), "\"AppState\"{}").unwrap();
    fs::write(steam.join("steamapps/common/Some Game/game.exe"), b"MZ").unwrap();
    // Broken client files that a repair replaces.
    fs::create_dir_all(steam.join("bin")).unwrap();
    fs::write(steam.join("steam.exe"), b"MZ").unwrap();
    fs::write(steam.join("bin/steamwebhelper.exe"), b"MZ").unwrap();
    steam
}

#[test]
fn preserve_and_restore_round_trip_keeps_user_data() {
    let tmp = tempdir().unwrap();
    let steam = fixture_steam_dir(tmp.path());
    let holding = tmp.path().join("pfx.repair-data");

    let preserved = preserve_steam_data_dirs(&steam, &holding).unwrap();
    assert_eq!(preserved, ["userdata", "config", "steamapps"]);

    // The data is out of the Steam dir (NSIS needs the destination clean of it)…
    assert!(!steam.join("userdata").exists());
    assert!(!steam.join("config").exists());
    assert!(!steam.join("steamapps").exists());
    // …sitting intact in the holding folder…
    assert!(holding.join("userdata/12345678/config/localconfig.vdf").is_file());
    assert!(holding.join("steamapps/common/Some Game/game.exe").is_file());
    // …and the client files were not touched by preservation.
    assert!(steam.join("steam.exe").is_file());
    assert!(steam.join("bin/steamwebhelper.exe").is_file());

    let restored = restore_steam_data_dirs(&holding, &steam).unwrap();
    assert_eq!(restored, ["userdata", "config", "steamapps"]);

    // Full round-trip: contents are back where they started.
    assert_eq!(
        fs::read_to_string(steam.join("config/loginusers.vdf")).unwrap(),
        "\"users\"{}"
    );
    assert!(steam.join("userdata/12345678/config/localconfig.vdf").is_file());
    assert!(steam.join("steamapps/appmanifest_620.acf").is_file());
    assert!(steam.join("steamapps/common/Some Game/game.exe").is_file());
    // The emptied holding folder is cleaned up.
    assert!(!holding.exists());
}

#[test]
fn restore_replaces_dirs_the_fresh_install_recreated() {
    let tmp = tempdir().unwrap();
    let steam = fixture_steam_dir(tmp.path());
    let holding = tmp.path().join("pfx.repair-data");
    preserve_steam_data_dirs(&steam, &holding).unwrap();

    // Simulate the fresh install re-creating `config` with installer defaults.
    fs::create_dir_all(steam.join("config")).unwrap();
    fs::write(steam.join("config/config.vdf"), "\"InstallConfigStore\"{}").unwrap();

    restore_steam_data_dirs(&holding, &steam).unwrap();

    // The preserved copy (with the logins) wins over the installer default.
    assert!(steam.join("config/loginusers.vdf").is_file());
    assert!(!steam.join("config/config.vdf").exists());
}

#[test]
fn preserve_skips_missing_dirs() {
    let tmp = tempdir().unwrap();
    let steam = tmp.path().join("Steam");
    fs::create_dir_all(steam.join("userdata/1")).unwrap();
    fs::write(steam.join("userdata/1/x.vdf"), "x").unwrap();
    let holding = tmp.path().join("holding");

    let preserved = preserve_steam_data_dirs(&steam, &holding).unwrap();
    assert_eq!(preserved, ["userdata"]);
    assert!(holding.join("userdata/1/x.vdf").is_file());

    let restored = restore_steam_data_dirs(&holding, &steam).unwrap();
    assert_eq!(restored, ["userdata"]);
    assert!(steam.join("userdata/1/x.vdf").is_file());
}

#[test]
fn preserve_with_no_data_dirs_creates_no_holding_folder() {
    let tmp = tempdir().unwrap();
    let steam = tmp.path().join("Steam");
    fs::create_dir_all(steam.join("bin")).unwrap();
    let holding = tmp.path().join("holding");

    let preserved = preserve_steam_data_dirs(&steam, &holding).unwrap();
    assert!(preserved.is_empty());
    assert!(!holding.exists());
}

#[test]
fn restore_from_missing_holding_dir_is_a_noop() {
    let tmp = tempdir().unwrap();
    let steam = tmp.path().join("Steam");
    fs::create_dir_all(&steam).unwrap();

    let restored = restore_steam_data_dirs(&tmp.path().join("nope"), &steam).unwrap();
    assert!(restored.is_empty());
}
