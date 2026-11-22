use super::super::*;

fn mtime_secs(path: &Path) -> u64 {
    std::fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn stamps_the_cloud_mtime() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("save.dat");
    std::fs::write(&path, b"x").unwrap();
    transfer::set_file_mtime_secs(&path, 1_700_000_000);
    assert_eq!(mtime_secs(&path), 1_700_000_000);
}

#[cfg(unix)]
#[test]
fn stamps_read_only_files_too() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ro.dat");
    std::fs::write(&path, b"x").unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&path, perms).unwrap();
    transfer::set_file_mtime_secs(&path, 1_600_000_000);
    assert_eq!(mtime_secs(&path), 1_600_000_000);
}
