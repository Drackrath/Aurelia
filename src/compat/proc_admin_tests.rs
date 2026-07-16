use super::*;

#[test]
fn recognises_aurelia_executable_names() {
    assert!(is_aurelia_name("aurelia"));
    assert!(is_aurelia_name("aurelia.exe"));
    assert!(is_aurelia_name("Aurelia.EXE"));
    assert!(!is_aurelia_name("aurelia-helper"));
    assert!(!is_aurelia_name("steam.exe"));
}

#[test]
fn distinguishes_daemon_from_admin_and_clients() {
    let s = |args: &[&str]| args.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    assert!(cmd_is_daemon(&s(&["aurelia.exe", "daemon"])));
    assert!(cmd_is_daemon(&s(&["aurelia", "daemon", "--socket", "/tmp/x"])));
    // A `daemon stop|list` invocation is not itself a daemon.
    assert!(!cmd_is_daemon(&s(&["aurelia", "daemon", "stop"])));
    assert!(!cmd_is_daemon(&s(&["aurelia", "daemon", "list"])));
    // Thin clients / one-off commands.
    assert!(!cmd_is_daemon(&s(&["aurelia", "list"])));
    assert!(!cmd_is_daemon(&s(&["aurelia"])));
}
