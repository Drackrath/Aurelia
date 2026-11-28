use super::*;

/// Every `--help` text, by path.
fn render_all_help() -> String {
    use clap::CommandFactory;
    fn walk(cmd: &clap::Command, path: &mut Vec<String>, out: &mut String) {
        let mut argv = path.clone();
        argv.push("--help".into());
        let Err(err) = Cli::command().try_get_matches_from(&argv) else {
            panic!("--help must not parse: {argv:?}");
        };
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp, "{argv:?}");
        let text = err
            .render()
            .to_string()
            .replace(aurelia::core::version::DISPLAY, "<version>");
        out.push_str(&format!("===== {} =====\n{}\n", path.join(" "), text));
        let mut subs: Vec<&clap::Command> = cmd.get_subcommands().collect();
        subs.sort_by_key(|s| s.get_name().to_string());
        // clap's generated `help` has no --help
        for sub in subs.into_iter().filter(|s| s.get_name() != "help") {
            path.push(sub.get_name().to_string());
            walk(sub, path, out);
            path.pop();
        }
    }
    let mut root = Cli::command();
    root.build();
    let mut out = String::new();
    walk(&root, &mut vec!["aurelia".to_string()], &mut out);
    out
}

/// CLI guard: help must not drift.
#[test]
fn cli_help_matches_snapshot() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/cli_help.txt");
    let actual = render_all_help();
    if std::env::var_os("AURELIA_UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    // CRLF checkouts still match.
    let expected = std::fs::read_to_string(&path)
        .expect("missing cli_help.txt snapshot")
        .replace("\r\n", "\n");
    if actual != expected {
        let first = actual
            .lines()
            .zip(expected.lines())
            .position(|(a, e)| a != e)
            .unwrap_or(actual.lines().count().min(expected.lines().count()));
        panic!(
            "CLI help changed (first diff at line {}); \
             set AURELIA_UPDATE_SNAPSHOTS=1 only for intended changes",
            first + 1
        );
    }
}

#[test]
fn launch_targets_json_shape() {
    use aurelia::steam_client::LaunchTargetChoice;
    let targets = [
        LaunchTargetChoice {
            id: "windows-host",
            platform: "windows",
            steam_mode: Some("host"),
            flags: vec!["--windows".into(), "--steam".into()],
            available: false,
            missing: Some("host-steam"),
            default: false,
        },
        LaunchTargetChoice {
            id: "linux",
            platform: "linux",
            steam_mode: None,
            flags: vec![],
            available: true,
            missing: None,
            default: true,
        },
    ];
    let v = crate::app::commands::launch::launch_targets_json(570, &targets);
    assert_eq!(
        v,
        serde_json::json!({
            "app_id": 570,
            "targets": [
                {
                    "id": "windows-host", "platform": "windows", "steam_mode": "host",
                    "flags": ["--windows", "--steam"], "available": false,
                    "missing": "host-steam", "default": false
                },
                {
                    "id": "linux", "platform": "linux", "steam_mode": null,
                    "flags": [], "available": true, "missing": null, "default": true
                }
            ]
        })
    );
}

#[test]
fn launch_options_json_shape() {
    use aurelia::steam_client::LaunchOptionInfo;
    let options = [LaunchOptionInfo {
        id: "0".into(),
        description: "Play".into(),
        executable: "game.exe".into(),
        arguments: "-novid".into(),
        working_dir: "bin".into(),
        oslist: "windows".into(),
        osarch: "64".into(),
        launch_type: "default".into(),
    }];
    let v = crate::app::commands::info::launch_options_json(570, &options, |o| o.id == "0");
    assert_eq!(
        v,
        serde_json::json!({
            "app_id": 570,
            "launch_options": [{
                "id": "0", "description": "Play", "executable": "game.exe",
                "arguments": "-novid", "working_dir": "bin", "oslist": "windows",
                "osarch": "64", "type": "default", "installed": true
            }]
        })
    );
}
