use super::{find_on_path, steam_client_installed};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

fn make_exec(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn empty_path() -> OsString {
    OsString::new()
}

#[test]
fn data_only_root_is_not_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Steam");
    std::fs::create_dir_all(root.join("steamapps")).unwrap();
    let flatpak: Vec<PathBuf> = vec![tmp.path().join("no-flatpak")];
    assert!(!steam_client_installed(
        &root,
        Some(&empty_path()),
        &flatpak
    ));
    assert!(!steam_client_installed(&root, None, &[]));
}

#[test]
fn root_with_client_files_is_installed() {
    for rel in ["steam.sh", "ubuntu12_32/steam"] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Steam");
        make_exec(&root.join(rel));
        assert!(steam_client_installed(&root, None, &[]), "{rel}");
    }
}

#[test]
fn steam_on_path_is_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    make_exec(&bin.join("steam"));
    let path_env = std::env::join_paths([tmp.path().join("empty"), bin.clone()]).unwrap();
    let root = tmp.path().join("Steam");
    assert!(steam_client_installed(&root, Some(&path_env), &[]));
    assert_eq!(
        find_on_path("steam", Some(&path_env)),
        Some(bin.join("steam"))
    );
}

#[cfg(unix)]
#[test]
fn non_executable_on_path_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("steam"), "").unwrap();
    let path_env = std::env::join_paths([bin]).unwrap();
    assert_eq!(find_on_path("steam", Some(&path_env)), None);
}

#[test]
fn flatpak_app_dir_is_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let flatpak = tmp.path().join("flatpak/app/com.valvesoftware.Steam");
    std::fs::create_dir_all(&flatpak).unwrap();
    let root = tmp.path().join("Steam");
    assert!(steam_client_installed(&root, None, &[flatpak]));
}
