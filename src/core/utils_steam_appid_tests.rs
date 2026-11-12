use super::ensure_steam_appid;

#[test]
fn writes_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    assert!(ensure_steam_appid(dir.path(), 440).unwrap());
    let content = std::fs::read_to_string(dir.path().join("steam_appid.txt")).unwrap();
    assert_eq!(content, "440");
}

#[test]
fn keeps_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("steam_appid.txt");
    std::fs::write(&path, "480\n").unwrap();
    assert!(!ensure_steam_appid(dir.path(), 440).unwrap());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "480\n");
}

#[test]
fn missing_dir_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope");
    assert!(ensure_steam_appid(&missing, 440).is_err());
}

#[cfg(unix)]
#[test]
fn read_only_dir_is_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let ro = dir.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Root ignores directory permissions.
    let writable = std::fs::write(ro.join("probe"), "x").is_ok();
    if !writable {
        assert!(ensure_steam_appid(&ro, 440).is_err());
    }
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
}
