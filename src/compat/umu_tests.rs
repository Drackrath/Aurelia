use super::*;

#[test]
fn finds_entry_in_base_dir() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(ENTRY_NAME), b"#!/bin/sh\n").unwrap();
    let root = find_entry_root(tmp.path()).unwrap();
    assert_eq!(root, tmp.path());
    assert_eq!(entry_point(&root), tmp.path().join(ENTRY_NAME));
}

#[test]
fn finds_entry_in_subdir() {
    // The tarball's own top-level directory holds `umu-run`.
    let tmp = tempfile::tempdir().unwrap();
    let sub = tmp.path().join("umu-launcher-1.2.3");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join(ENTRY_NAME), b"#!/bin/sh\n").unwrap();
    let root = find_entry_root(tmp.path()).unwrap();
    assert_eq!(root, sub);
}

#[test]
fn custom_path_accepts_direct_binary() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join(ENTRY_NAME);
    std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
    let inst = installed(Some(&bin)).unwrap();
    assert_eq!(inst.entry, bin);
    assert_eq!(inst.root, tmp.path());
    assert_eq!(inst.version, "custom");
}

#[test]
fn custom_path_accepts_directory() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(ENTRY_NAME), b"#!/bin/sh\n").unwrap();
    let inst = installed(Some(tmp.path())).unwrap();
    assert_eq!(inst.entry, tmp.path().join(ENTRY_NAME));
}

#[test]
fn missing_entry_is_none() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(find_entry_root(tmp.path()).is_none());
    assert!(installed(Some(tmp.path())).is_none());
}

#[test]
fn selects_tarball_over_checksum() {
    // Mirrors the asset-picking logic in `latest_release` without the network.
    let assets = [
        ("umu-launcher.tar.gz.sha256", false),
        ("umu-launcher.tar.gz", true),
    ];
    let chosen = assets
        .iter()
        .find(|(name, _)| name.ends_with(".tar.gz") && !name.contains(".sha"));
    assert_eq!(chosen.map(|(_, ok)| *ok), Some(true));
}
