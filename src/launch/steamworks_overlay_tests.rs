use crate::core::config::LauncherConfig;
use crate::infra::runners::CommandSpec;
use crate::launch::pipeline::LaunchErrorKind;
use crate::launch::steamworks_overlay::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn plan(home: &Path, sdk32: Option<PathBuf>, mirror: Vec<MirrorEntry>) -> OverlayPlan {
    OverlayPlan {
        bwrap: PathBuf::from("/usr/bin/bwrap"),
        home: home.to_path_buf(),
        libs: ClientLibs {
            sdk64: PathBuf::from("/opt/sw/sdk64/steamclient.so"),
            sdk32,
        },
        mirror,
        app_id: 480,
        account_id: Some(7),
        daemon_socket: None,
    }
}

fn spec(program: &str, args: &[&str]) -> CommandSpec {
    CommandSpec {
        program: PathBuf::from(program),
        args: args.iter().map(|a| a.to_string()).collect(),
        cwd: Some(PathBuf::from("/games/x")),
        env: HashMap::new(),
    }
}

/// Position of `seq` in `args`.
fn find_seq(args: &[String], seq: &[&str]) -> Option<usize> {
    args.windows(seq.len())
        .position(|w| w.iter().zip(seq).all(|(a, b)| a == b))
}

#[test]
fn resolve_libs_accepts_single_file() {
    let dir = tempfile::tempdir().unwrap();
    let so = dir.path().join("libsteamclient.so");
    write(&so, "x");
    let libs = resolve_client_libs(&so).unwrap();
    assert_eq!(libs.sdk64, so);
    assert_eq!(libs.sdk32, None);
}

#[test]
fn resolve_libs_reads_sdk_directories() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("sdk64/steamclient.so"), "64");
    let libs = resolve_client_libs(dir.path()).unwrap();
    assert_eq!(libs.sdk64, dir.path().join("sdk64/steamclient.so"));
    assert_eq!(libs.sdk32, None);

    write(&dir.path().join("sdk32/steamclient.so"), "32");
    let libs = resolve_client_libs(dir.path()).unwrap();
    assert_eq!(libs.sdk32, Some(dir.path().join("sdk32/steamclient.so")));
}

#[test]
fn resolve_libs_fails_without_sdk64() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("sdk32/steamclient.so"), "32");
    let err = resolve_client_libs(dir.path()).unwrap_err();
    assert!(matches!(err.kind, LaunchErrorKind::Dependency));
    assert!(err.message.contains("sdk64/steamclient.so"));
}

#[test]
fn scan_skips_overlaid_entries_and_keeps_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let steam = dir.path();
    std::os::unix::fs::symlink("/somewhere/Steam", steam.join("root")).unwrap();
    std::fs::create_dir(steam.join("debian-installation")).unwrap();
    std::fs::create_dir(steam.join("sdk64")).unwrap();
    std::os::unix::fs::symlink("/x/linux32", steam.join("sdk32")).unwrap();
    write(&steam.join("steam.pid"), "1");
    write(&steam.join("registry.vdf"), "");

    let mirror = scan_steam_dir(steam);
    assert_eq!(
        mirror,
        vec![
            MirrorEntry {
                name: "debian-installation".into(),
                kind: MirrorKind::Bind,
            },
            MirrorEntry {
                name: "root".into(),
                kind: MirrorKind::Symlink(PathBuf::from("/somewhere/Steam")),
            },
        ]
    );
}

#[test]
fn scan_of_missing_dir_is_empty() {
    assert!(scan_steam_dir(Path::new("/nonexistent/aurelia/.steam")).is_empty());
}

#[test]
fn registry_vdf_wraps_pid_and_libraries() {
    let (head, tail) = registry_vdf_parts(Path::new("/home/u/.steam"), Some(42));
    let text = format!("{head}1234{tail}");
    assert!(text.starts_with("\"Registry\"\n{\n\t\"HKCU\""));
    assert!(text.contains("\"ActiveProcess\""));
    assert!(text.contains("\"pid\"\t\t\"1234\"\n"));
    assert!(text.contains("\"SteamClientDll\"\t\t\"/home/u/.steam/sdk32/steamclient.so\""));
    assert!(text.contains("\"SteamClientDll64\"\t\t\"/home/u/.steam/sdk64/steamclient.so\""));
    assert!(text.contains("\"ActiveUser\"\t\t\"42\""));
    assert_eq!(text.matches('{').count(), text.matches('}').count());
}

#[test]
fn registry_vdf_omits_unknown_user_and_escapes() {
    let (head, tail) = registry_vdf_parts(Path::new("/home/a\"b/.steam"), None);
    let text = format!("{head}1{tail}");
    assert!(!text.contains("ActiveUser"));
    assert!(text.contains("/home/a\\\"b/.steam/sdk64/steamclient.so"));
}

#[test]
fn bwrap_args_overlay_only_steam_dir() {
    let mirror = vec![
        MirrorEntry {
            name: "root".into(),
            kind: MirrorKind::Symlink(PathBuf::from("/data/Steam")),
        },
        MirrorEntry {
            name: "debian-installation".into(),
            kind: MirrorKind::Bind,
        },
    ];
    let p = plan(Path::new("/home/u"), None, mirror);
    let args = bwrap_args(&p, Some(Path::new("/games/x")));

    assert_eq!(&args[..3], ["--dev-bind", "/", "/"]);
    let tmpfs = find_seq(&args, &["--tmpfs", "/home/u/.steam"]).unwrap();
    let link = find_seq(&args, &["--symlink", "/data/Steam", "/home/u/.steam/root"]).unwrap();
    let bind = find_seq(
        &args,
        &[
            "--bind",
            "/home/u/.steam/debian-installation",
            "/home/u/.steam/debian-installation",
        ],
    )
    .unwrap();
    let lib = find_seq(
        &args,
        &[
            "--ro-bind",
            "/opt/sw/sdk64/steamclient.so",
            "/home/u/.steam/sdk64/steamclient.so",
        ],
    )
    .unwrap();
    assert!(tmpfs < link && tmpfs < bind && tmpfs < lib);
    assert!(find_seq(&args, &["--dir", "/home/u/.steam/sdk64"]).unwrap() < lib);
    assert!(!args.iter().any(|a| a.contains("sdk32")));
    assert!(find_seq(&args, &["--chdir", "/games/x"]).is_some());
    // Only the tmpfs touches ~/.steam.
    assert!(
        !args
            .iter()
            .any(|a| a == "/home/u" || a == "/home/u/.steam/steam.pid")
    );
}

#[test]
fn bwrap_args_add_sdk32_when_present() {
    let p = plan(
        Path::new("/home/u"),
        Some(PathBuf::from("/opt/sw/sdk32/steamclient.so")),
        Vec::new(),
    );
    let args = bwrap_args(&p, None);
    assert!(
        find_seq(
            &args,
            &[
                "--ro-bind",
                "/opt/sw/sdk32/steamclient.so",
                "/home/u/.steam/sdk32/steamclient.so",
            ],
        )
        .is_some()
    );
    assert!(!args.iter().any(|a| a == "--chdir"));
}

#[test]
fn wrap_command_execs_original_after_wrapper() {
    let mut original = spec("/games/x/game.bin", &["-v", "--fast"]);
    original.env.insert("FOO".into(), "bar".into());
    original
        .env
        .insert("PROTON_DISABLE_LSTEAMCLIENT".into(), "1".into());
    let p = plan(Path::new("/home/u"), None, Vec::new());
    let wrapped = wrap_command(&original, &p);

    assert_eq!(wrapped.program, PathBuf::from("/usr/bin/bwrap"));
    assert_eq!(wrapped.cwd, original.cwd);
    let sep = wrapped.args.iter().position(|a| a == "--").unwrap();
    let tail = &wrapped.args[sep + 1..];
    assert_eq!(&tail[..2], ["/bin/sh", "-c"]);
    assert_eq!(tail[3], "aurelia-steamworks");
    assert_eq!(tail[4], "/home/u/.steam");
    assert!(tail[5].ends_with("\"pid\"\t\t\""));
    assert!(tail[6].contains("\"ActiveUser\"\t\t\"7\""));
    assert_eq!(&tail[7..], ["/games/x/game.bin", "-v", "--fast"]);

    assert_eq!(wrapped.env["SteamAppId"], "480");
    assert_eq!(wrapped.env["SteamGameId"], "480");
    assert_eq!(wrapped.env["FOO"], "bar");
    assert!(!wrapped.env.contains_key("PROTON_DISABLE_LSTEAMCLIENT"));
}

#[test]
fn wrap_command_keeps_existing_app_ids() {
    let mut original = spec("/g", &[]);
    original.env.insert("SteamAppId".into(), "1".into());
    original.env.insert("SteamGameId".into(), "2".into());
    let wrapped = wrap_command(&original, &plan(Path::new("/h"), None, Vec::new()));
    assert_eq!(wrapped.env["SteamAppId"], "1");
    assert_eq!(wrapped.env["SteamGameId"], "2");
}

#[test]
fn wrapper_script_writes_live_pid_then_execs() {
    let dir = tempfile::tempdir().unwrap();
    let steam = dir.path().join(".steam");
    std::fs::create_dir(&steam).unwrap();
    let p = plan(dir.path(), None, Vec::new());
    let wrapped = wrap_command(&spec("/bin/sh", &["-c", "echo $$; echo \"$0\""]), &p);
    let sep = wrapped.args.iter().position(|a| a == "--").unwrap();
    let tail = &wrapped.args[sep + 1..];

    // Run without bwrap, on a temp dir.
    let out = Command::new(&tail[0]).args(&tail[1..]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let mut lines = stdout.lines();
    let pid = lines.next().unwrap();
    assert_eq!(lines.next(), Some("/bin/sh"));

    let pid_file = std::fs::read_to_string(steam.join("steam.pid")).unwrap();
    assert_eq!(pid_file, format!("{pid}\n"));
    let registry = std::fs::read_to_string(steam.join("registry.vdf")).unwrap();
    assert!(registry.contains(&format!("\"pid\"\t\t\"{pid}\"\n")));
}

#[test]
fn wrapper_script_fails_when_dir_unwritable() {
    let p = plan(Path::new("/nonexistent/aurelia-home"), None, Vec::new());
    let wrapped = wrap_command(&spec("/bin/true", &[]), &p);
    let sep = wrapped.args.iter().position(|a| a == "--").unwrap();
    let tail = &wrapped.args[sep + 1..];
    let status = Command::new(&tail[0])
        .args(&tail[1..])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(127));
}

#[test]
fn only_native_and_proton_runners_supported() {
    assert!(runner_supported("Native"));
    assert!(runner_supported("Wine-TKG"));
    assert!(!runner_supported("Luxtorpeda"));
}

#[test]
fn stand_in_is_off_by_default() {
    let mut config = LauncherConfig::default();
    assert!(!config.steamworks_client);
    assert!(!crate::infra::runners::wine_tkg::stand_in_client_active(
        &config
    ));
    config.steamworks_client = true;
    assert!(crate::infra::runners::wine_tkg::stand_in_client_active(
        &config
    ));
}

#[test]
fn plan_fails_closed_without_library() {
    let dir = tempfile::tempdir().unwrap();
    let config = LauncherConfig {
        steamworks_client: true,
        steamworks_client_path: Some(dir.path().join("missing").to_string_lossy().into()),
        ..Default::default()
    };
    let mut s = spec("/g", &[]);
    s.env
        .insert("HOME".into(), dir.path().to_string_lossy().into());
    let err = plan_for(&config, &s, 480, None).unwrap_err();
    assert!(matches!(err.kind, LaunchErrorKind::Dependency));
    assert!(!dir.path().join(".steam").exists());
}

#[test]
fn plan_uses_spec_home_and_mirrors_it() {
    if find_bwrap().is_none() {
        eprintln!("skipping: bwrap not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let so = dir.path().join("lib/sdk64/steamclient.so");
    write(&so, "stand-in");
    write(&home.join(".steam/registry.vdf"), "real");
    std::os::unix::fs::symlink("/data/Steam", home.join(".steam/root")).unwrap();
    let config = LauncherConfig {
        steamworks_client: true,
        steamworks_client_path: Some(dir.path().join("lib").to_string_lossy().into()),
        ..Default::default()
    };
    let mut s = spec("/g", &[]);
    s.env.insert("HOME".into(), home.to_string_lossy().into());

    let p = plan_for(&config, &s, 480, Some(9)).unwrap();
    assert_eq!(p.home, home);
    assert_eq!(p.libs.sdk64, so);
    assert_eq!(p.app_id, 480);
    assert_eq!(p.account_id, Some(9));
    assert_eq!(
        p.mirror,
        vec![MirrorEntry {
            name: "root".into(),
            kind: MirrorKind::Symlink(PathBuf::from("/data/Steam")),
        }]
    );
}

#[test]
fn plan_creates_only_empty_mount_point() {
    if find_bwrap().is_none() {
        eprintln!("skipping: bwrap not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let so = dir.path().join("libsteamclient.so");
    write(&so, "stand-in");
    let config = LauncherConfig {
        steamworks_client: true,
        steamworks_client_path: Some(so.to_string_lossy().into()),
        ..Default::default()
    };
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let mut s = spec("/g", &[]);
    s.env.insert("HOME".into(), home.to_string_lossy().into());
    plan_for(&config, &s, 1, None).unwrap();
    let steam = home.join(".steam");
    assert!(steam.is_dir());
    assert_eq!(std::fs::read_dir(&steam).unwrap().count(), 0);
}

/// Can this host run unprivileged bwrap?
fn bwrap_usable() -> Option<PathBuf> {
    let bwrap = find_bwrap()?;
    let ok = Command::new(&bwrap)
        .args(["--dev-bind", "/", "/", "/bin/true"])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    ok.then_some(bwrap)
}

#[test]
fn overlay_runs_under_bwrap_without_touching_real_dir() {
    let Some(bwrap) = bwrap_usable() else {
        eprintln!("skipping: bwrap unusable here");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let steam = home.join(".steam");
    let data = dir.path().join("data");
    write(&data.join("marker"), "data");
    write(&steam.join("sdk64/steamclient.so"), "real");
    write(&steam.join("steam.pid"), "1\n");
    write(&steam.join("keep/file"), "kept");
    std::os::unix::fs::symlink(&data, steam.join("root")).unwrap();
    let so = dir.path().join("stand-in.so");
    write(&so, "stand-in");
    let socket = dir.path().join("run/aurelia-test.sock");
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();

    let p = OverlayPlan {
        bwrap,
        home: home.clone(),
        libs: ClientLibs {
            sdk64: so,
            sdk32: None,
        },
        mirror: scan_steam_dir(&steam),
        app_id: 480,
        account_id: None,
        daemon_socket: Some(socket.clone()),
    };
    let script = "s=\"$HOME/.steam\"; cat \"$s/sdk64/steamclient.so\"; echo; \
                  [ \"$(cat \"$s/steam.pid\")\" = \"$$\" ] && echo pid-ok; \
                  grep -q \"\\\"pid\\\".*\\\"$$\\\"\" \"$s/registry.vdf\" && echo reg-ok; \
                  cat \"$s/root/marker\" \"$s/keep/file\"; echo; echo \"$SteamAppId\"; \
                  [ -S \"$AURELIA_DAEMON_SOCKET\" ] && echo sock-ok";
    let mut original = spec("/bin/sh", &["-c", script]);
    original.cwd = Some(dir.path().to_path_buf());
    original
        .env
        .insert("HOME".into(), home.to_string_lossy().into());
    let wrapped = wrap_command(&original, &p);
    let out = Command::new(&wrapped.program)
        .args(&wrapped.args)
        .envs(&wrapped.env)
        .current_dir(wrapped.cwd.as_ref().unwrap())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout, "stand-in\npid-ok\nreg-ok\ndatakept\n480\nsock-ok\n");

    // Real directory is untouched.
    assert_eq!(
        std::fs::read_to_string(steam.join("sdk64/steamclient.so")).unwrap(),
        "real"
    );
    assert_eq!(
        std::fs::read_to_string(steam.join("steam.pid")).unwrap(),
        "1\n"
    );
    assert!(!steam.join("registry.vdf").exists());
}

#[test]
fn wrap_command_passes_daemon_socket() {
    let mut p = plan(Path::new("/home/u"), None, Vec::new());
    p.daemon_socket = Some(PathBuf::from("/run/user/1000/aurelia-1000-ab.sock"));
    let mut original = spec("/games/x/game", &[]);
    original
        .env
        .insert("PRESSURE_VESSEL_FILESYSTEMS_RW".into(), "/mnt/games".into());
    let wrapped = wrap_command(&original, &p);
    assert_eq!(
        wrapped.env.get(DAEMON_SOCKET_ENV).map(String::as_str),
        Some("/run/user/1000/aurelia-1000-ab.sock")
    );
    assert_eq!(
        wrapped
            .env
            .get("PRESSURE_VESSEL_FILESYSTEMS_RW")
            .map(String::as_str),
        Some("/mnt/games:/run/user/1000/aurelia-1000-ab.sock")
    );
}

#[test]
fn daemon_socket_is_shared_once() {
    let mut env = HashMap::new();
    let socket = Path::new("/tmp/a.sock");
    add_daemon_socket(&mut env, socket);
    add_daemon_socket(&mut env, socket);
    assert_eq!(env["PRESSURE_VESSEL_FILESYSTEMS_RW"], "/tmp/a.sock");
    assert_eq!(env[DAEMON_SOCKET_ENV], "/tmp/a.sock");
}

#[test]
fn no_socket_leaves_env_alone() {
    let p = plan(Path::new("/home/u"), None, Vec::new());
    let wrapped = wrap_command(&spec("/games/x/game", &[]), &p);
    assert!(!wrapped.env.contains_key(DAEMON_SOCKET_ENV));
    assert!(!wrapped.env.contains_key("PRESSURE_VESSEL_FILESYSTEMS_RW"));
}
