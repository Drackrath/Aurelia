use super::super::*;
use crate::core::models::LibraryGame;
use crate::steam_client::{LaunchInfo, LaunchTarget};

fn mac_launch(install: &Path, executable: &str) -> LaunchContext {
    LaunchContext {
        app: LibraryGame {
            app_id: 250900,
            name: "Test".to_string(),
            install_path: Some(install.to_string_lossy().into_owned()),
            is_installed: true,
            playtime_forever_minutes: None,
            active_branch: "public".to_string(),
            update_available: false,
            steam_config_mismatch: false,
            update_queued: false,
            local_manifest_ids: HashMap::new(),
            is_owned: true,
            is_family_shared: false,
            online_required: None,
            platform: None,
            from_windows_steam: false,
            size_on_disk: None,
        },
        launch_info: LaunchInfo {
            app_id: 250900,
            id: "0".into(),
            description: "Test".into(),
            executable: executable.into(),
            arguments: String::new(),
            workingdir: None,
            target: LaunchTarget::NativeMacos,
        },
        launcher_config: crate::core::config::LauncherConfig::default(),
        user_config: None,
        proton_path: None,
        steam_enabled: false,
        use_umu: false,
        umu_run: None,
        target_architecture: Default::default(),
        dll_resolutions: Vec::new(),
        game_fixups: Default::default(),
        verification_ptr: std::ptr::null_mut(),
    }
}

#[cfg(target_os = "macos")]
fn write_bundle(install: &Path, name: &str, entries: &str, binaries: &[&str]) -> PathBuf {
    let bundle = install.join(name);
    let binary_dir = bundle.join("Contents/MacOS");
    std::fs::create_dir_all(&binary_dir).unwrap();
    for binary in binaries {
        std::fs::write(binary_dir.join(binary), "").unwrap();
    }
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>{entries}</dict></plist>
"#
    );
    std::fs::write(bundle.join("Contents/Info.plist"), plist).unwrap();
    bundle
}

fn bundle_with(binaries: &[&str]) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = tmp.path().join("Game.app");
    for binary in binaries {
        let path = bundle.join("Contents").join(binary);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }
    (tmp, bundle)
}

fn named(name: &str) -> BundleExecutable {
    BundleExecutable::Named(name.to_string())
}

#[test]
fn bundle_binary_follows_subfolders_inside_the_bundle() {
    let (_tmp, bundle) = bundle_with(&["MacOS/bin/game", "Resources/run.sh"]);

    assert_eq!(
        bundle_binary(&bundle, named("bin/game")),
        Some(bundle.join("Contents/MacOS/bin/game"))
    );
    assert_eq!(
        bundle_binary(&bundle, named("../Resources/run.sh")),
        Some(bundle.join("Contents/MacOS/../Resources/run.sh"))
    );
}

#[test]
fn bundle_binary_rejects_files_outside_the_bundle() {
    let (tmp, bundle) = bundle_with(&["MacOS/Game"]);
    std::fs::write(tmp.path().join("outside"), "").unwrap();

    assert_eq!(bundle_binary(&bundle, named("../../../outside")), None);
}

#[test]
fn bundle_binary_rejects_missing_or_invalid_executables() {
    let (_tmp, bundle) = bundle_with(&["MacOS/Game"]);

    assert_eq!(bundle_binary(&bundle, named("love")), None);
    assert_eq!(bundle_binary(&bundle, named(".")), None);
}

#[cfg(unix)]
#[test]
fn bundle_binary_follows_symlinks_only_inside_the_bundle() {
    let (tmp, bundle) = bundle_with(&["Resources/game"]);
    std::fs::write(tmp.path().join("outside"), "").unwrap();
    let binaries = bundle.join("Contents/MacOS");
    std::fs::create_dir_all(&binaries).unwrap();
    std::os::unix::fs::symlink("../Resources/game", binaries.join("inside")).unwrap();
    std::os::unix::fs::symlink(tmp.path().join("outside"), binaries.join("escape")).unwrap();

    assert_eq!(
        bundle_binary(&bundle, named("inside")),
        Some(binaries.join("inside"))
    );
    assert_eq!(bundle_binary(&bundle, named("escape")), None);
}

#[tokio::test]
async fn native_runner_keeps_a_path_inside_a_bundle() {
    let tmp = tempfile::tempdir().unwrap();
    let binary = tmp.path().join("Game.macOS.app/Contents/MacOS/Game.macOS");
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::write(&binary, "").unwrap();

    let ctx = mac_launch(tmp.path(), "./Game.macOS.app/Contents/MacOS/Game.macOS");
    let spec = NativeRunner.build_command(&ctx).await.unwrap();

    assert_eq!(spec.program, binary);
    assert_eq!(spec.cwd.as_deref(), Some(tmp.path()));
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn native_runner_runs_the_named_bundle_executable() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = write_bundle(
        tmp.path(),
        "The Binding of Isaac Rebirth.app",
        "<key>CFBundleExecutable</key><string>The Binding of Isaac Afterbirth+</string>",
        &[
            "The Binding of Isaac Rebirth",
            "The Binding of Isaac Afterbirth+",
            "crash_reporter",
        ],
    );

    let ctx = mac_launch(tmp.path(), "The Binding of Isaac Rebirth.app");
    let spec = NativeRunner.build_command(&ctx).await.unwrap();

    assert_eq!(
        spec.program,
        bundle.join("Contents/MacOS/The Binding of Isaac Afterbirth+")
    );
    assert_eq!(spec.cwd.as_deref(), Some(Path::new("/")));
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn native_runner_reads_binary_info_plists() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = write_bundle(
        tmp.path(),
        "Balatro.app",
        "<key>CFBundleExecutable</key><string>love</string>",
        &["love"],
    );
    let converted = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "binary1"])
        .arg(bundle.join("Contents/Info.plist"))
        .status()
        .unwrap();
    assert!(converted.success());

    let ctx = mac_launch(tmp.path(), "Balatro.app");
    let spec = NativeRunner.build_command(&ctx).await.unwrap();

    assert_eq!(spec.program, bundle.join("Contents/MacOS/love"));
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn native_runner_runs_the_bundle_name_without_an_executable_key() {
    let tmp = tempfile::tempdir().unwrap();
    let bundles = [
        ("NoKey", "<key>CFBundleName</key><string>Game</string>"),
        ("Empty", "<key>CFBundleExecutable</key><string></string>"),
    ];
    for (name, entries) in bundles {
        let bundle = write_bundle(tmp.path(), &format!("{name}.app"), entries, &[name]);

        let ctx = mac_launch(tmp.path(), &format!("{name}.app"));
        let spec = NativeRunner.build_command(&ctx).await.unwrap();

        assert_eq!(spec.program, bundle.join("Contents/MacOS").join(name));
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn native_runner_runs_the_bundle_name_without_an_info_plist() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = write_bundle(tmp.path(), "Game.app", "", &["Game"]);
    std::fs::remove_file(bundle.join("Contents/Info.plist")).unwrap();

    let ctx = mac_launch(tmp.path(), "Game.app");
    let spec = NativeRunner.build_command(&ctx).await.unwrap();

    assert_eq!(spec.program, bundle.join("Contents/MacOS/Game"));
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn native_runner_rereads_info_plist_after_an_update() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = mac_launch(tmp.path(), "Isaac.app");
    for binary in ["Rebirth", "Afterbirth+"] {
        let entries = format!("<key>CFBundleExecutable</key><string>{binary}</string>");
        let bundle = write_bundle(
            tmp.path(),
            "Isaac.app",
            &entries,
            &["Rebirth", "Afterbirth+"],
        );

        let spec = NativeRunner.build_command(&ctx).await.unwrap();

        assert_eq!(spec.program, bundle.join("Contents/MacOS").join(binary));
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn native_runner_rejects_a_bundle_macos_would_not_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let bundles = [
        (
            "Number",
            "<key>CFBundleExecutable</key><integer>7</integer>",
        ),
        (
            "Shell",
            "<key>CFBundleExecutable</key><string>/bin/sh</string>",
        ),
    ];
    for (name, entries) in bundles {
        // The bundle-name binary exists, so falling back to it would launch.
        write_bundle(tmp.path(), &format!("{name}.app"), entries, &[name]);

        let ctx = mac_launch(tmp.path(), &format!("{name}.app"));
        let error = NativeRunner.build_command(&ctx).await.unwrap_err();

        assert_eq!(error.kind, LaunchErrorKind::GameData, "{name}");
    }
}
