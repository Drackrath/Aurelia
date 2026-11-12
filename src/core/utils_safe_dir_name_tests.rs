use super::{is_safe_dir_name, resolves_within, safe_relative_path};
use std::path::{Path, PathBuf};

#[test]
fn accepts_plain_names() {
    for name in [
        "Game A",
        "Half-Life 2",
        "GE-Proton9-20",
        "Proton 9.0",
        ".hidden",
    ] {
        assert!(is_safe_dir_name(name), "{name:?}");
    }
}

#[test]
fn rejects_paths_and_dot_names() {
    for name in [
        "",
        " ",
        ".",
        "..",
        "/root",
        "a/b",
        "a\\b",
        "C:\\Games",
        "../x",
        "a/",
    ] {
        assert!(!is_safe_dir_name(name), "{name:?}");
    }
}

#[cfg(windows)]
#[test]
fn rejects_drive_prefix() {
    assert!(!is_safe_dir_name("C:"));
}

#[test]
fn relative_paths_keep_plain_parts() {
    assert_eq!(
        safe_relative_path("bin/game"),
        Some(PathBuf::from("bin/game"))
    );
    assert_eq!(safe_relative_path("./a/./b"), Some(PathBuf::from("a/b")));
}

#[test]
fn relative_paths_reject_escapes() {
    for path in ["", ".", "../x", "a/../../x", "/etc/passwd", "a\0b"] {
        assert_eq!(safe_relative_path(path), None, "{path:?}");
    }
}

#[cfg(unix)]
#[test]
fn symlinked_ancestor_out_of_root_is_caught() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    std::os::unix::fs::symlink(outside.path(), root_path.join("link")).unwrap();
    assert!(resolves_within(
        &root_path,
        &root_path.join("plain/new/file")
    ));
    assert!(!resolves_within(&root_path, &root_path.join("link/file")));
    assert!(!resolves_within(&root_path, Path::new("/")));
}
