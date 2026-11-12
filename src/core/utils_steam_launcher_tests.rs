use super::{steam_launcher_program, steam_running_in};
use std::path::Path;

fn make_exec(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
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
        steam_launcher_program(Some(&path_env), Some(&root)),
        Some(bin.join("steam"))
    );
}

#[test]
fn falls_back_to_steam_sh() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Steam");
    make_exec(&root.join("steam.sh"));
    let path_env = std::env::join_paths([tmp.path().join("bin")]).unwrap();
    assert_eq!(
        steam_launcher_program(Some(&path_env), Some(&root)),
        Some(root.join("steam.sh"))
    );
}

#[test]
fn no_launcher_gives_none() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Steam");
    std::fs::create_dir_all(&root).unwrap();
    assert_eq!(steam_launcher_program(None, Some(&root)), None);
    assert_eq!(steam_launcher_program(None, None), None);
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
