use super::*;
use std::collections::HashMap;

/// Typical Proton command before options.
fn proton_spec() -> CommandSpec {
    CommandSpec {
        program: PathBuf::from("/opt/proton/proton"),
        args: vec!["waitforexitandrun".into(), "Game.exe".into()],
        cwd: Some(PathBuf::from("/games/demo")),
        env: HashMap::from([
            ("WINEDLLOVERRIDES".to_string(), "dxgi=n".to_string()),
            ("PROTON_LOG".to_string(), "0".to_string()),
        ]),
    }
}

fn wrapped(line: &str) -> CommandSpec {
    let mut spec = proton_spec();
    wrap_command(&mut spec, &parse_launch_options(line).expect(line));
    spec
}

#[test]
fn blank_line_leaves_command_alone() {
    for line in ["", " \t "] {
        assert!(parse_launch_options(line).unwrap().is_empty());
        let spec = wrapped(line);
        assert_eq!(spec.program, PathBuf::from("/opt/proton/proton"));
        assert_eq!(spec.args, ["waitforexitandrun", "Game.exe"]);
        assert_eq!(spec.env, proton_spec().env);
    }
}

#[test]
fn plain_words_are_appended_to_the_game() {
    let spec = wrapped(r#"-windowed -lang 'pt BR'"#);
    assert_eq!(spec.program, PathBuf::from("/opt/proton/proton"));
    assert_eq!(
        spec.args,
        [
            "waitforexitandrun",
            "Game.exe",
            "-windowed",
            "-lang",
            "pt BR"
        ]
    );
}

#[test]
fn single_wrapper_receives_the_full_command() {
    let spec = wrapped("strace -f %command% +connect 127.0.0.1");
    assert_eq!(spec.program, PathBuf::from("strace"));
    assert_eq!(
        spec.args,
        [
            "-f",
            "/opt/proton/proton",
            "waitforexitandrun",
            "Game.exe",
            "+connect",
            "127.0.0.1"
        ]
    );
    assert_eq!(spec.cwd, Some(PathBuf::from("/games/demo")));
}

#[test]
fn chained_wrappers_keep_their_order() {
    let spec = wrapped("taskset -c 0-3 nice -n 5 %command%");
    assert_eq!(spec.program, PathBuf::from("taskset"));
    assert_eq!(
        spec.args,
        [
            "-c",
            "0-3",
            "nice",
            "-n",
            "5",
            "/opt/proton/proton",
            "waitforexitandrun",
            "Game.exe"
        ]
    );
}

#[test]
fn assignments_set_and_override_environment() {
    let spec = wrapped(r#"PROTON_LOG=1 SDL_VIDEODRIVER='x11' %command%"#);
    assert_eq!(spec.program, PathBuf::from("/opt/proton/proton"));
    assert_eq!(spec.env["PROTON_LOG"], "1");
    assert_eq!(spec.env["SDL_VIDEODRIVER"], "x11");
    assert_eq!(spec.env["WINEDLLOVERRIDES"], "dxgi=n");
}

#[test]
fn assignment_after_wrapper_is_a_wrapper_argument() {
    let options = parse_launch_options("env A=1 %command%").unwrap();
    assert!(options.env.is_empty());
    assert_eq!(options.wrappers, ["env", "A=1"]);
}

#[test]
fn mangohud_switch_never_reaches_the_game() {
    assert_eq!(
        wrapped("-mangohud -high").args,
        ["waitforexitandrun", "Game.exe", "-high"]
    );
    assert_eq!(
        wrapped("%command% -high --mangohud").args,
        ["waitforexitandrun", "Game.exe", "-high"]
    );
}

#[test]
fn malformed_lines_are_errors() {
    for line in [
        "-name \"Player",
        "-name 'Player",
        "%command% -x %command%",
        "-path C:\\",
    ] {
        assert!(parse_launch_options(line).is_err(), "{line}");
    }
}

#[test]
fn quoting_matches_posix_shell_words() {
    assert_eq!(
        split_command_line(r#"one  'two three'"four"  five\ six "7\$8" '\n'"#).unwrap(),
        ["one", "two threefour", "five six", "7$8", "\\n"]
    );
    assert_eq!(
        split_command_line(r#"empty="" ''"#).unwrap(),
        ["empty=", ""]
    );
}

#[test]
fn only_shell_names_count_as_assignments() {
    assert_eq!(env_assignment("_X9=a=b"), Some(("_X9", "a=b")));
    assert_eq!(env_assignment("9X=a"), None);
    assert_eq!(env_assignment("--opt=a"), None);
    assert_eq!(env_assignment("plain"), None);
}

#[test]
fn wrapper_must_exist_on_path_or_disk() {
    let bin = tempfile::tempdir().unwrap();
    std::fs::write(bin.path().join("fakewrap"), "").unwrap();
    let path = std::env::join_paths([Path::new("/nonexistent"), bin.path()]).unwrap();
    assert!(wrapper_exists("fakewrap", Some(&path)));
    assert!(!wrapper_exists("nowrap", Some(&path)));
    assert!(!wrapper_exists("fakewrap", None));
    let absolute = bin.path().join("fakewrap");
    assert!(wrapper_exists(absolute.to_str().unwrap(), None));
}

/// Spawns a real process from it.
#[cfg(unix)]
#[test]
fn wrapped_spec_spawns_correctly() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("report.txt");
    let executable = |name: &str, body: String| {
        let path = dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    // Each line: one observed value.
    let game = executable(
        "game.sh",
        format!(
            "#!/bin/sh\n{{ echo \"mode=$RUN_MODE\"; echo \"outer=$OUTER\"; \
             for a in \"$@\"; do echo \"arg=$a\"; done; }} > '{}'\n",
            report.display()
        ),
    );
    let outer = executable(
        "outer.sh",
        "#!/bin/sh\nOUTER=\"$1\"; shift; export OUTER; exec \"$@\"\n".to_string(),
    );

    let mut spec = CommandSpec {
        program: game,
        args: vec!["--level".into(), "two words".into()],
        cwd: None,
        env: HashMap::from([("RUN_MODE".to_string(), "runner".to_string())]),
    };
    let line = format!("RUN_MODE=user {} tagged %command% -fast", outer.display());
    wrap_command(&mut spec, &parse_launch_options(&line).unwrap());

    let status = std::process::Command::new(&spec.program)
        .args(&spec.args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .envs(&spec.env)
        .status()
        .unwrap();
    assert!(status.success());
    let lines: Vec<String> = std::fs::read_to_string(&report)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        lines,
        [
            "mode=user",
            "outer=tagged",
            "arg=--level",
            "arg=two words",
            "arg=-fast"
        ]
    );
}
