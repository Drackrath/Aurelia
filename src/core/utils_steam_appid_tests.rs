use super::{ensure_steam_appid, write_appid_or_remove};

struct FailingWriter;

impl std::io::Write for FailingWriter {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("disk full"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn failed_write_removes_partial_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("steam_appid.txt");
    std::fs::write(&path, "").unwrap();
    let err = write_appid_or_remove(&path, FailingWriter, 440).unwrap_err();
    assert_eq!(err.to_string(), "disk full");
    assert!(!path.exists());
    // Next launch retries and writes.
    assert!(ensure_steam_appid(dir.path(), 440).unwrap());
}

#[test]
fn successful_write_keeps_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("steam_appid.txt");
    let file = std::fs::File::create(&path).unwrap();
    write_appid_or_remove(&path, file, 70).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "70");
}

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
