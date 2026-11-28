use super::super::*;

const APP_ID: u32 = 4242001;

#[test]
fn finds_process_carrying_the_app_id() {
    // Re-run this test binary as the game: Apple binaries hide their
    // environment on macOS, so a system tool can't stand in for it.
    if std::env::var("SteamAppId").ok() == Some(APP_ID.to_string()) {
        std::thread::sleep(std::time::Duration::from_secs(30));
        return;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "compat::running::tests::app_processes::finds_process_carrying_the_app_id",
        ])
        .env("SteamAppId", APP_ID.to_string())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let found = processes_for_app(APP_ID);
    let prefix = processes_for_app(APP_ID / 10);
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(found.contains(&child.id()));
    assert!(!prefix.contains(&child.id()));
}
