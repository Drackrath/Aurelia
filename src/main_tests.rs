use super::*;

#[test]
fn human_bytes_scales_units() {
    assert_eq!(human_bytes(0), "0 B");
    assert_eq!(human_bytes(512), "512 B");
    assert_eq!(human_bytes(1024), "1.00 KiB");
    assert_eq!(human_bytes(1536), "1.50 KiB");
    assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.00 GiB");
}

#[test]
fn eta_formats_hms() {
    assert_eq!(format_eta(0), "00:00:00");
    assert_eq!(format_eta(83), "00:01:23");
    assert_eq!(format_eta(3661), "01:01:01");
}

#[test]
fn rate_tracker_estimates_speed_and_eta() {
    let mut r = RateTracker::new();
    // First sample only primes the tracker (no prior point).
    let (s0, e0) = r.sample(0, 1000);
    assert_eq!(s0, 0.0);
    assert!(e0.is_none());
    // Force a measurable interval, then feed more bytes.
    std::thread::sleep(std::time::Duration::from_millis(120));
    let (s1, e1) = r.sample(200, 1000);
    assert!(s1 > 0.0, "speed should be positive after progress");
    assert!(e1.is_some(), "eta should be estimable once moving");
}

#[test]
fn rate_tracker_resets() {
    let mut r = RateTracker::new();
    let _ = r.sample(100, 1000);
    r.reset();
    let (s, e) = r.sample(0, 1000);
    assert_eq!(s, 0.0);
    assert!(e.is_none());
}

/// Parse an argv into a `Cli` the way the binary does.
fn parse(args: &[&str]) -> Cli {
    Cli::try_parse_from(args).expect("args should parse")
}

#[test]
fn install_positional_and_subcommands_coexist() {
    // Positional install (Heroic depends on this form).
    match parse(&["aurelia", "install", "945360"]).command {
        Command::Install(args) => {
            assert!(args.action.is_none());
            assert_eq!(args.app_id, Some(945360));
        }
        _ => panic!("expected Install"),
    }
    // Management verbs.
    match parse(&["aurelia", "install", "list"]).command {
        Command::Install(args) => assert!(matches!(args.action, Some(InstallAction::List))),
        _ => panic!("expected Install list"),
    }
    match parse(&["aurelia", "install", "stop", "945360"]).command {
        Command::Install(args) => {
            assert!(matches!(args.action, Some(InstallAction::Stop { app_id: 945360 })))
        }
        _ => panic!("expected Install stop"),
    }
}

#[test]
fn interactive_login_runs_locally_not_forwarded() {
    // The regression: these were forwarded to the daemon, where rpassword ran
    // without a tty and the password was echoed in clear text.
    assert!(must_run_locally(&parse(&["aurelia", "login"])));
    assert!(must_run_locally(&parse(&["aurelia", "login", "-u", "me"])));
    assert!(must_run_locally(&parse(&["aurelia", "login", "--qr"])));
    assert!(must_run_locally(&parse(&["aurelia", "login", "--code"])));
    assert!(must_run_locally(&parse(&["aurelia", "login", "--pin"])));
}

#[test]
fn daemon_oriented_and_json_login_still_forward() {
    // These need the daemon (or are non-tty), so they must NOT be pinned local.
    assert!(!must_run_locally(&parse(&["aurelia", "login", "--health"])));
    assert!(!must_run_locally(&parse(&["aurelia", "login", "--reconnect"])));
    assert!(!must_run_locally(&parse(&["aurelia", "--json", "login", "-u", "me"])));
    assert!(!must_run_locally(&parse(&["aurelia", "login", "--json", "--qr"])));
}

#[test]
fn ordinary_commands_forward_but_local_managers_do_not() {
    assert!(!must_run_locally(&parse(&["aurelia", "list"])));
    assert!(!must_run_locally(&parse(&["aurelia", "logout"])));
    // Process/daemon managers must run in-process.
    assert!(must_run_locally(&parse(&["aurelia", "kill"])));
    assert!(must_run_locally(&parse(&["aurelia", "daemon", "stop"])));
    // Bare `aurelia daemon` (becomes the server) is handled earlier, before the
    // forward gate, so it is not flagged local here.
    assert!(!must_run_locally(&parse(&["aurelia", "daemon"])));
}
