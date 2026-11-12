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
fn rate_tracker_averages_bursts_over_the_window() {
    use std::time::{Duration, Instant};
    let mut r = RateTracker::new();
    let t0 = Instant::now();
    // 10 MB/s, but delivered in 2 s bursts.
    let mut speeds = Vec::new();
    for tick in 0..=80u64 {
        let at = t0 + Duration::from_millis(tick * 250);
        let bytes = (tick / 8) * 20_000_000;
        speeds.push(r.sample_at(at, bytes, 10_000_000_000).0);
    }
    for s in &speeds[24..] {
        assert!((6e6..=14e6).contains(s), "speed {s} outside 6-14 MB/s");
    }
}

#[test]
fn rate_tracker_restarts_when_counter_drops() {
    use std::time::{Duration, Instant};
    let mut r = RateTracker::new();
    let t0 = Instant::now();
    r.sample_at(t0, 5_000_000, 100_000_000);
    r.sample_at(t0 + Duration::from_secs(1), 6_000_000, 100_000_000);
    // A retried depot resets the counters.
    let (s, _) = r.sample_at(t0 + Duration::from_secs(2), 1_000_000, 100_000_000);
    assert!(s >= 0.0);
    let (s, _) = r.sample_at(t0 + Duration::from_secs(3), 3_000_000, 100_000_000);
    assert!((1.9e6..=2.1e6).contains(&s), "{s}");
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
fn hide_uninstalled_launch_options_takes_on_or_off() {
    let name = "hide-uninstalled-launch-options";
    assert!(Cli::try_parse_from(["aurelia", "config", name]).is_ok());
    assert!(Cli::try_parse_from(["aurelia", "config", name, "on"]).is_ok());
    assert!(Cli::try_parse_from(["aurelia", "config", name, "off"]).is_ok());
    assert!(Cli::try_parse_from(["aurelia", "config", name, "maybe"]).is_err());
}

#[test]
fn play_accepts_a_launch_option_id() {
    // Heroic combines it with target flags.
    let argv = [
        "aurelia",
        "play",
        "312200",
        "--platform",
        "windows",
        "--launch-option",
        "3",
    ];
    match parse(&argv).command {
        Command::Play { launch_option, .. } => assert_eq!(launch_option.as_deref(), Some("3")),
        _ => panic!("expected play"),
    }
    match parse(&["aurelia", "play", "312200"]).command {
        Command::Play { launch_option, .. } => assert!(launch_option.is_none()),
        _ => panic!("expected play"),
    }
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
            assert!(matches!(
                args.action,
                Some(InstallAction::Stop { app_id: 945360 })
            ))
        }
        _ => panic!("expected Install stop"),
    }
    match parse(&["aurelia", "install", "pause", "945360"]).command {
        Command::Install(args) => {
            assert!(matches!(
                args.action,
                Some(InstallAction::Pause { app_id: 945360 })
            ))
        }
        _ => panic!("expected Install pause"),
    }
    match parse(&["aurelia", "install", "resume", "--all"]).command {
        Command::Install(args) => {
            assert!(matches!(
                args.action,
                Some(InstallAction::Resume {
                    app_id: None,
                    all: true
                })
            ))
        }
        _ => panic!("expected Install resume"),
    }
    match parse(&["aurelia", "install", "forget", "945360", "--delete-files"]).command {
        Command::Install(args) => {
            assert!(matches!(
                args.action,
                Some(InstallAction::Forget {
                    app_id: 945360,
                    delete_files: true
                })
            ))
        }
        _ => panic!("expected Install forget"),
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
    assert!(!must_run_locally(&parse(&[
        "aurelia",
        "login",
        "--reconnect"
    ])));
    assert!(!must_run_locally(&parse(&[
        "aurelia", "--json", "login", "-u", "me"
    ])));
    assert!(!must_run_locally(&parse(&[
        "aurelia", "login", "--json", "--qr"
    ])));
}

#[test]
fn qr_timeout_parses_and_requires_qr() {
    match parse(&["aurelia", "login", "--qr", "--timeout", "90"]).command {
        Command::Login(args) => assert_eq!(args.timeout, Some(90)),
        _ => panic!("expected Login"),
    }
    assert!(Cli::try_parse_from(["aurelia", "login", "--timeout", "90"]).is_err());
    assert!(Cli::try_parse_from(["aurelia", "login", "--qr", "--timeout", "0"]).is_err());
    // Driver QR logins still forward.
    assert!(!must_run_locally(&parse(&[
        "aurelia", "--json", "login", "--qr", "-t", "5"
    ])));
}

#[test]
fn ordinary_commands_forward_but_local_managers_do_not() {
    assert!(!must_run_locally(&parse(&["aurelia", "list"])));
    assert!(!must_run_locally(&parse(&["aurelia", "logout"])));
    // Process/daemon managers must run in-process.
    assert!(must_run_locally(&parse(&["aurelia", "kill"])));
    assert!(must_run_locally(&parse(&[
        "aurelia",
        "--json",
        "capabilities"
    ])));
    assert!(must_run_locally(&parse(&[
        "aurelia",
        "capabilities",
        "config",
        "game"
    ])));
    assert!(must_run_locally(&parse(&["aurelia", "daemon", "stop"])));
    // Bare `aurelia daemon` (becomes the server) is handled earlier, before the
    // forward gate, so it is not flagged local here.
    assert!(!must_run_locally(&parse(&["aurelia", "daemon"])));
}

#[test]
fn cli_has_no_conflicting_flags() {
    use clap::CommandFactory;
    crate::cli::Cli::command().debug_assert();
}

#[test]
fn download_limit_parses_numbers_and_off() {
    assert_eq!(crate::cli::parse_download_limit("8000"), Ok(8000));
    assert_eq!(crate::cli::parse_download_limit(" 0 "), Ok(0));
    for word in ["off", "OFF", "none", "unlimited"] {
        assert_eq!(crate::cli::parse_download_limit(word), Ok(0), "{word}");
    }
    assert!(crate::cli::parse_download_limit("1 MB").is_err());
    assert!(crate::cli::parse_download_limit("-5").is_err());
}

#[test]
fn download_limit_reads_in_bytes() {
    use crate::commands::config::describe_download_limit;
    assert_eq!(describe_download_limit(0), "unlimited");
    assert_eq!(describe_download_limit(8000), "8000 kbit/s (1.0 MB/s)");
    assert_eq!(describe_download_limit(800), "800 kbit/s (100.0 kB/s)");
}

#[test]
fn download_limit_command_parses() {
    use clap::Parser;
    let cli =
        crate::cli::Cli::try_parse_from(["aurelia", "config", "download-limit", "off"]).unwrap();
    assert!(matches!(
        cli.command,
        crate::cli::Command::Config {
            command: crate::cli::ConfigCommand::DownloadLimit { kbit: Some(0) }
        }
    ));
}

#[test]
fn anticheat_auto_install_takes_on_or_off() {
    let name = "anticheat-auto-install";
    assert!(Cli::try_parse_from(["aurelia", "config", name]).is_ok());
    assert!(Cli::try_parse_from(["aurelia", "config", name, "off"]).is_ok());
    assert!(Cli::try_parse_from(["aurelia", "config", name, "maybe"]).is_err());
}

#[test]
fn proton_anticheat_takes_eac_or_battleye() {
    assert!(Cli::try_parse_from(["aurelia", "proton", "anticheat", "eac"]).is_ok());
    assert!(Cli::try_parse_from(["aurelia", "proton", "anticheat", "battleye"]).is_ok());
    assert!(Cli::try_parse_from(["aurelia", "proton", "anticheat", "vac"]).is_err());
}

#[test]
fn relative_path_args_resolve_against_cwd() {
    let cwd = std::env::current_dir().unwrap();
    let mut cli = Cli::try_parse_from(["aurelia", "image", "570", "-o", "x.jpg"]).unwrap();
    absolutize_path_args(&mut cli.command);
    let Command::Image { output, .. } = cli.command else {
        panic!("expected image");
    };
    assert_eq!(output, Some(cwd.join("x.jpg")));

    // Runner names are not paths.
    let mut cli =
        Cli::try_parse_from(["aurelia", "play", "570", "--proton", "GE-Proton9-20"]).unwrap();
    absolutize_path_args(&mut cli.command);
    let Command::Play { proton, .. } = cli.command else {
        panic!("expected play");
    };
    assert_eq!(proton.as_deref(), Some("GE-Proton9-20"));

    let mut cli =
        Cli::try_parse_from(["aurelia", "play", "570", "--proton", "./runners/p"]).unwrap();
    absolutize_path_args(&mut cli.command);
    let Command::Play { proton, .. } = cli.command else {
        panic!("expected play");
    };
    let expected = cwd.join("./runners/p").to_string_lossy().into_owned();
    assert_eq!(proton, Some(expected));
}
