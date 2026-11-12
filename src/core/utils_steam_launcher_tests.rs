use super::{
    STEAM_FLATPAK_APP_ID, SteamLauncher, SteamRootKind, steam_launcher, steam_root_kind,
    steam_running_in,
};
use std::path::{Path, PathBuf};

fn make_exec(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn plain(program: PathBuf) -> Option<SteamLauncher> {
    Some(SteamLauncher {
        program,
        prefix_args: Vec::new(),
    })
}

fn flatpak_root(home: &Path) -> PathBuf {
    home.join(format!(
        ".var/app/{STEAM_FLATPAK_APP_ID}/.local/share/Steam"
    ))
}

#[test]
fn steam_on_path_wins() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    make_exec(&bin.join("steam"));
    let root = tmp.path().join("Steam");
    make_exec(&root.join("steam.sh"));
    let path_env = std::env::join_paths([&bin]).unwrap();
    assert_eq!(
        steam_launcher(Some(&path_env), Some(&root), &[]),
        plain(bin.join("steam"))
    );
}

#[test]
fn falls_back_to_steam_sh() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Steam");
    make_exec(&root.join("steam.sh"));
    let path_env = std::env::join_paths([tmp.path().join("bin")]).unwrap();
    assert_eq!(
        steam_launcher(Some(&path_env), Some(&root), &[]),
        plain(root.join("steam.sh"))
    );
}

#[test]
fn no_launcher_gives_none() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Steam");
    std::fs::create_dir_all(&root).unwrap();
    assert_eq!(steam_launcher(None, Some(&root), &[]), None);
    assert_eq!(steam_launcher(None, None, &[]), None);
}

#[test]
fn flatpak_root_never_uses_steam_sh() {
    let tmp = tempfile::tempdir().unwrap();
    let root = flatpak_root(tmp.path());
    make_exec(&root.join("steam.sh"));
    assert_eq!(steam_root_kind(&root), SteamRootKind::Flatpak);
    assert_eq!(steam_launcher(None, Some(&root), &[]), None);
}

#[test]
fn snap_root_never_uses_steam_sh() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("snap/steam/common/.local/share/Steam");
    make_exec(&root.join("steam.sh"));
    assert_eq!(steam_root_kind(&root), SteamRootKind::Snap);
    assert_eq!(steam_launcher(None, Some(&root), &[]), None);
}

#[test]
fn symlink_into_flatpak_is_sandboxed() {
    let tmp = tempfile::tempdir().unwrap();
    let real = flatpak_root(tmp.path());
    make_exec(&real.join("steam.sh"));
    let link = tmp.path().join("steam-link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(steam_root_kind(&link), SteamRootKind::Flatpak);
    assert_eq!(steam_launcher(None, Some(&link), &[]), None);
}

#[test]
fn flatpak_root_runs_through_flatpak() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    make_exec(&bin.join("flatpak"));
    // Host steam must not win.
    make_exec(&bin.join("steam"));
    let root = flatpak_root(tmp.path());
    std::fs::create_dir_all(&root).unwrap();
    let path_env = std::env::join_paths([&bin]).unwrap();
    assert_eq!(
        steam_launcher(Some(&path_env), Some(&root), &[]),
        Some(SteamLauncher {
            program: bin.join("flatpak"),
            prefix_args: vec!["run".into(), STEAM_FLATPAK_APP_ID.into()],
        })
    );
}

#[test]
fn flatpak_app_dir_used_as_last_resort() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    make_exec(&bin.join("flatpak"));
    let app_dir = tmp
        .path()
        .join(format!("flatpak/app/{STEAM_FLATPAK_APP_ID}"));
    std::fs::create_dir_all(&app_dir).unwrap();
    let path_env = std::env::join_paths([&bin]).unwrap();
    let got = steam_launcher(Some(&path_env), None, &[app_dir]).unwrap();
    assert_eq!(got.program, bin.join("flatpak"));
    // Without the app dir: none.
    assert_eq!(steam_launcher(Some(&path_env), None, &[]), None);
}

fn fake_proc(root: &Path, pid: u32, comm: &str, state: &str) {
    let dir = root.join(pid.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
    std::fs::write(
        dir.join("status"),
        format!("Name:\t{comm}\nState:\t{state}\nPid:\t{pid}\n"),
    )
    .unwrap();
}

#[test]
fn zombie_steam_is_not_running() {
    let tmp = tempfile::tempdir().unwrap();
    fake_proc(tmp.path(), 4242, "steam", "Z (zombie)");
    let pid_file = tmp.path().join("steam.pid");
    std::fs::write(&pid_file, "4242\n").unwrap();
    assert!(!steam_running_in(tmp.path(), Some(&pid_file)));
}

#[test]
fn live_steam_is_running() {
    let tmp = tempfile::tempdir().unwrap();
    fake_proc(tmp.path(), 100, "bash", "S (sleeping)");
    assert!(!steam_running_in(tmp.path(), None));
    fake_proc(tmp.path(), 4243, "steam", "S (sleeping)");
    assert!(steam_running_in(tmp.path(), None));
}

#[test]
fn wine_steam_exe_is_not_host_steam() {
    let tmp = tempfile::tempdir().unwrap();
    fake_proc(tmp.path(), 5000, "steam.exe", "S (sleeping)");
    assert!(!steam_running_in(tmp.path(), None));
}

#[test]
fn live_pid_file_is_running() {
    let tmp = tempfile::tempdir().unwrap();
    fake_proc(tmp.path(), 77, "reaper", "S (sleeping)");
    let pid_file = tmp.path().join("steam.pid");
    std::fs::write(&pid_file, "77").unwrap();
    assert!(steam_running_in(tmp.path(), Some(&pid_file)));
}
