use super::*;

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
fn cli_has_no_conflicting_flags() {
    use clap::CommandFactory;
    crate::app::cli::Cli::command().debug_assert();
}

#[test]
fn download_limit_parses_numbers_and_off() {
    assert_eq!(crate::app::cli::parse_download_limit("8000"), Ok(8000));
    assert_eq!(crate::app::cli::parse_download_limit(" 0 "), Ok(0));
    for word in ["off", "OFF", "none", "unlimited"] {
        assert_eq!(crate::app::cli::parse_download_limit(word), Ok(0), "{word}");
    }
    assert!(crate::app::cli::parse_download_limit("1 MB").is_err());
    assert!(crate::app::cli::parse_download_limit("-5").is_err());
}

#[test]
fn download_limit_reads_in_bytes() {
    use crate::app::commands::config::describe_download_limit;
    assert_eq!(describe_download_limit(0), "unlimited");
    assert_eq!(describe_download_limit(8000), "8000 kbit/s (1.0 MB/s)");
    assert_eq!(describe_download_limit(800), "800 kbit/s (100.0 kB/s)");
}

#[test]
fn download_limit_command_parses() {
    use clap::Parser;
    let cli = crate::app::cli::Cli::try_parse_from(["aurelia", "config", "download-limit", "off"])
        .unwrap();
    assert!(matches!(
        cli.command,
        crate::app::cli::Command::Config {
            command: crate::app::cli::ConfigCommand::DownloadLimit { kbit: Some(0) }
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
fn config_cloud_sync_takes_on_or_off() {
    for (arg, want) in [
        (None, None),
        (Some("on"), Some(true)),
        (Some("off"), Some(false)),
    ] {
        let mut argv = vec!["aurelia", "config", "cloud-sync"];
        argv.extend(arg);
        match parse(&argv).command {
            Command::Config {
                command: ConfigCommand::CloudSync { mode },
            } => assert_eq!(mode.map(|m| matches!(m, OnOffArg::On)), want),
            _ => panic!("expected config cloud-sync"),
        }
    }
    assert!(Cli::try_parse_from(["aurelia", "config", "cloud-sync", "auto"]).is_err());
}

/// `config game` cloud-sync value, parsed.
fn game_cloud_sync(extra: &[&str]) -> Option<CloudSyncArg> {
    let mut argv = vec!["aurelia", "config", "game", "792100"];
    argv.extend_from_slice(extra);
    match parse(&argv).command {
        Command::Config {
            command: ConfigCommand::Game { args },
        } => args.cloud_sync,
        _ => panic!("expected config game"),
    }
}

#[test]
fn config_game_cloud_sync_parses_auto_on_off() {
    assert_eq!(game_cloud_sync(&[]), None);
    let on = game_cloud_sync(&["--cloud-sync", "on"]);
    assert_eq!(on.and_then(CloudSyncArg::as_override), Some(true));
    let off = game_cloud_sync(&["--cloud-sync", "off"]);
    assert_eq!(off.and_then(CloudSyncArg::as_override), Some(false));
    let auto = game_cloud_sync(&["--cloud-sync", "auto"]);
    assert_eq!(auto, Some(CloudSyncArg::Auto));
    assert_eq!(auto.and_then(CloudSyncArg::as_override), None);
    let argv = ["aurelia", "config", "game", "1", "--cloud-sync", "maybe"];
    assert!(Cli::try_parse_from(argv).is_err());
    let argv = [
        "aurelia",
        "config",
        "game",
        "1",
        "--clear",
        "--cloud-sync",
        "on",
    ];
    assert!(Cli::try_parse_from(argv).is_err());
}
