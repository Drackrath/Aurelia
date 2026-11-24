use super::*;

#[test]
fn config_display_flags_merge_and_clear() {
    let cli = parse(&[
        "aurelia",
        "config",
        "display",
        "--tty",
        "tty3",
        "--display",
        "",
    ]);
    let Command::Config {
        command: ConfigCommand::Display { target, clear },
    } = cli.command
    else {
        panic!("expected config display");
    };
    assert!(!clear);
    let mut current = aurelia::core::config::DisplayTarget {
        display: Some(":0".into()),
        ..Default::default()
    };
    target.apply_to(&mut current);
    assert_eq!(current.tty.as_deref(), Some("tty3"));
    // Empty value clears the field.
    assert_eq!(current.display, None);
    assert!(
        Cli::try_parse_from(["aurelia", "config", "display", "--clear", "--tty", "3"]).is_err()
    );
}

#[test]
fn config_game_display_and_wayland_flags() {
    let cli = parse(&[
        "aurelia",
        "config",
        "game",
        "792100",
        "--wayland-display",
        "wayland-4",
        "--wayland-native",
        "on",
    ]);
    let Command::Config {
        command: ConfigCommand::Game { args },
    } = cli.command
    else {
        panic!("expected config game");
    };
    assert_eq!(
        args.display_target.wayland_display.as_deref(),
        Some("wayland-4")
    );
    assert_eq!(
        args.wayland_native.and_then(WaylandNativeArg::as_override),
        Some(true)
    );
    assert!(
        Cli::try_parse_from([
            "aurelia",
            "config",
            "game",
            "1",
            "--no-display",
            "--tty",
            "3"
        ])
        .is_err()
    );
}

#[test]
fn config_wayland_native_takes_on_or_off() {
    assert!(Cli::try_parse_from(["aurelia", "config", "wayland-native"]).is_ok());
    assert!(Cli::try_parse_from(["aurelia", "config", "wayland-native", "on"]).is_ok());
    assert!(Cli::try_parse_from(["aurelia", "config", "wayland-native", "auto"]).is_err());
}
