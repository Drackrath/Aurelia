use super::*;

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

    // Steam-runtime runner: path, name, clear.
    let runner_of = |arg: &str| {
        let mut cli =
            Cli::try_parse_from(["aurelia", "config", "steam-runtime-runner", arg]).unwrap();
        absolutize_path_args(&mut cli.command);
        let Command::Config {
            command: ConfigCommand::SteamRuntimeRunner { runner },
        } = cli.command
        else {
            panic!("expected steam-runtime-runner");
        };
        runner
    };
    let expected = cwd.join("wine/bin").to_string_lossy().into_owned();
    assert_eq!(runner_of("wine/bin"), Some(expected));
    assert_eq!(runner_of("GE-Proton9-20").as_deref(), Some("GE-Proton9-20"));
    assert_eq!(runner_of("").as_deref(), Some(""));

    // Game prefix resolves before forwarding.
    let mut cli =
        Cli::try_parse_from(["aurelia", "config", "game", "570", "--prefix", "rel"]).unwrap();
    absolutize_path_args(&mut cli.command);
    let Command::Config {
        command: ConfigCommand::Game { args },
    } = cli.command
    else {
        panic!("expected config game");
    };
    assert_eq!(args.prefix, Some(cwd.join("rel")));

    // Depot downloads land in client cwd.
    let mut cli =
        Cli::try_parse_from(["aurelia", "depots", "download", "1", "2", "a", "-o", "out"]).unwrap();
    absolutize_path_args(&mut cli.command);
    let Command::Depots(DepotsArgs {
        command: Some(DepotsCommand::Download { output, .. }),
        ..
    }) = cli.command
    else {
        panic!("expected depots download");
    };
    assert_eq!(output, Some(cwd.join("out")));
}
