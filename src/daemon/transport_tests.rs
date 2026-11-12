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

#[test]
fn endpoint_keyed_by_config_dir() {
    use super::{config_tag_for, unix_endpoint_for};
    use std::path::Path;
    let base = Path::new("/run/user/1000");
    let default = Path::new("/home/u/.config/Aurelia");

    // Default dir keeps the old name.
    let tag = config_tag_for(default, Some(default));
    assert_eq!(tag, None);
    assert_eq!(
        unix_endpoint_for(None, None, base, 1000),
        "/run/user/1000/aurelia-1000.sock"
    );

    let a1 = config_tag_for(Path::new("/tmp/a"), Some(default)).unwrap();
    let a2 = config_tag_for(Path::new("/tmp/a"), Some(default)).unwrap();
    let b = config_tag_for(Path::new("/tmp/b"), Some(default)).unwrap();
    assert_eq!(a1, a2);
    assert_ne!(a1, b);
    let ep = unix_endpoint_for(None, Some(&a1), base, 1000);
    assert_eq!(ep, format!("/run/user/1000/aurelia-1000-{a1}.sock"));

    // Explicit socket still wins.
    let ep = unix_endpoint_for(Some("/x.sock".into()), Some(&a1), base, 1000);
    assert_eq!(ep, "/x.sock");
}
