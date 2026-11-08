use super::imp::{dir_is_private, ensure_private_dir};
use std::os::unix::fs::PermissionsExt;

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn missing_dir_is_created_owner_only() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("aurelia-test");
    ensure_private_dir(&dir).unwrap();
    assert_eq!(mode(&dir), 0o700);
    assert!(dir_is_private(&dir));
}

#[test]
fn loose_dir_we_own_is_tightened() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("loose");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(!dir_is_private(&dir));
    ensure_private_dir(&dir).unwrap();
    assert_eq!(mode(&dir), 0o700);
}

#[test]
fn symlinked_dir_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(ensure_private_dir(&link).is_err());
    assert!(!dir_is_private(&link));
}
