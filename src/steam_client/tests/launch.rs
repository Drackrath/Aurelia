use super::super::*;

#[test]
fn select_launch_entry_prefers_installed_platform() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("hl.exe"), "x").unwrap();
    std::fs::write(tmp.path().join("hl.sh"), "x").unwrap();

    let entry = |id: &str, exe: &str, target: LaunchTarget| LaunchInfo {
        app_id: 70,
        id: id.to_string(),
        description: String::new(),
        executable: exe.to_string(),
        arguments: String::new(),
        workingdir: None,
        target,
    };
    let options = vec![
        entry("0", "hl.exe", LaunchTarget::WindowsProton),
        entry("1", "hl.sh", LaunchTarget::NativeLinux),
    ];
    let mut app = LibraryGame {
        app_id: 70,
        name: "Half-Life".to_string(),
        install_path: Some(tmp.path().to_string_lossy().to_string()),
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
        platform: Some("linux".to_string()),
        from_windows_steam: false,
        size_on_disk: None,
    };

    // Installed platform wins over declared order and stale files.
    let picked = launch::select_launch_entry(&options, &app, false, false).unwrap();
    assert_eq!(picked.id, "1");

    // Explicit Proton/Windows request still picks the Windows entry.
    let picked = launch::select_launch_entry(&options, &app, true, false).unwrap();
    assert_eq!(picked.id, "0");

    // Explicit native preference beats the windows manifest.
    app.platform = Some("windows".to_string());
    let picked = launch::select_launch_entry(&options, &app, false, true).unwrap();
    assert_eq!(picked.id, "1");

    // Unknown installed platform: first existing executable.
    app.platform = None;
    let picked = launch::select_launch_entry(&options, &app, false, false).unwrap();
    assert_eq!(picked.id, "0");
}

fn launch_option(id: &str, executable: &str, target: LaunchTarget) -> LaunchInfo {
    LaunchInfo {
        app_id: 312200,
        id: id.to_string(),
        description: String::new(),
        executable: executable.to_string(),
        arguments: String::new(),
        workingdir: None,
        target,
    }
}

#[test]
fn find_launch_option_returns_the_requested_entry() {
    let options = vec![
        launch_option("0", "Chasm.exe", LaunchTarget::WindowsProton),
        launch_option("3", "Chasm.exe", LaunchTarget::WindowsProton),
    ];

    assert_eq!(launch::find_launch_option(&options, "3").unwrap().id, "3");

    let error = launch::find_launch_option(&options, "9")
        .unwrap_err()
        .to_string();
    assert!(error.contains("macOS-only"), "{error}");
    assert!(error.contains("available: 0, 3"), "{error}");
}

#[test]
fn launch_option_target_conflicts_are_rejected() {
    let linux = launch_option("1", "game.sh", LaunchTarget::NativeLinux);
    let windows = launch_option("0", "game.exe", LaunchTarget::WindowsProton);

    // No explicit flag: stored preferences lose.
    assert!(launch::check_launch_option_target(&linux, None).is_ok());
    assert!(launch::check_launch_option_target(&windows, None).is_ok());

    let matching = Some((LaunchTarget::NativeLinux, "--platform linux"));
    assert!(launch::check_launch_option_target(&linux, matching).is_ok());

    let error = launch::check_launch_option_target(&windows, matching)
        .unwrap_err()
        .to_string();
    assert!(error.contains("--platform linux"), "{error}");
    assert!(error.contains("starts the Windows build"), "{error}");

    let proton = Some((LaunchTarget::WindowsProton, "--proton"));
    let error = launch::check_launch_option_target(&linux, proton)
        .unwrap_err()
        .to_string();
    assert!(error.contains("--proton"), "{error}");
    assert!(error.contains("starts the native Linux build"), "{error}");
}

#[test]
fn launch_option_must_be_installed() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("Game.exe"), b"").unwrap();
    let app = LibraryGame {
        app_id: 312200,
        name: "Chasm".to_string(),
        install_path: Some(tmp.path().to_string_lossy().into_owned()),
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
        platform: Some("windows".to_string()),
        from_windows_steam: false,
        size_on_disk: None,
    };

    let installed = launch_option("0", "game.exe", LaunchTarget::WindowsProton);
    assert!(launch::require_launch_option_installed(&installed, &app).is_ok());

    let missing = launch_option("1", "game.sh", LaunchTarget::NativeLinux);
    let error = launch::require_launch_option_installed(&missing, &app)
        .unwrap_err()
        .to_string();
    assert!(error.contains("isn't installed"), "{error}");
    assert!(error.contains("launch-targets 312200"), "{error}");

    let empty = launch_option("2", "", LaunchTarget::WindowsProton);
    assert!(launch::require_launch_option_installed(&empty, &app).is_err());

    // Steam manifests use backslashes.
    std::fs::create_dir(tmp.path().join("x64")).unwrap();
    std::fs::write(tmp.path().join("x64/Game.exe"), b"").unwrap();
    assert!(launch::launch_executable_installed(&app, "X64\\game.exe"));
    assert!(!launch::launch_executable_installed(
        &app,
        "Debug\\game.exe"
    ));

    // Installed games lose missing entries.
    let info = |id: &str, executable: &str| LaunchOptionInfo {
        id: id.to_string(),
        executable: executable.to_string(),
        ..Default::default()
    };
    let all = vec![
        info("0", "x64\\Game.exe"),
        info("1", "Debug\\Game.exe"),
        info("2", ""),
    ];
    let mut options = all.clone();
    launch::hide_uninstalled_launch_options(&mut options, &app);
    let kept: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(kept, ["0"]);

    // Leftover files don't count.
    let mut uninstalled = app;
    uninstalled.is_installed = false;
    // Uninstalled games keep every entry.
    let mut options = all.clone();
    launch::hide_uninstalled_launch_options(&mut options, &uninstalled);
    assert_eq!(options.len(), all.len());
    assert!(!launch::launch_executable_installed(
        &uninstalled,
        "game.exe"
    ));
}

#[test]
fn select_launch_entry_matches_executable_case_insensitively() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("bin/x64")).unwrap();
    std::fs::write(tmp.path().join("bin/x64/factorio.exe"), "x").unwrap();
    std::fs::write(tmp.path().join("bin/x64/factorio"), "x").unwrap();

    let entry = |id: &str, exe: &str, wd: Option<&str>, target: LaunchTarget| LaunchInfo {
        app_id: 427520,
        id: id.to_string(),
        description: String::new(),
        executable: exe.to_string(),
        arguments: String::new(),
        workingdir: wd.map(str::to_string),
        target,
    };
    let options = vec![
        entry(
            "1",
            "bin\\x64\\Factorio.exe",
            Some("BIN/X64"),
            LaunchTarget::WindowsProton,
        ),
        entry("3", "bin/x64/factorio", None, LaunchTarget::NativeLinux),
    ];
    let app = LibraryGame {
        app_id: 427520,
        name: "Factorio".to_string(),
        install_path: Some(tmp.path().to_string_lossy().to_string()),
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
        platform: Some("linux".to_string()),
        from_windows_steam: false,
        size_on_disk: None,
    };

    // Proton request finds the lowercase exe.
    let picked = launch::select_launch_entry(&options, &app, true, false).unwrap();
    assert_eq!(picked.id, "1");
    // Compared as paths: the on-disk spelling uses the host's separator.
    assert_eq!(
        std::path::Path::new(&picked.executable),
        std::path::Path::new("bin/x64/factorio.exe")
    );
    assert_eq!(
        picked.workingdir.as_deref().map(std::path::Path::new),
        Some(std::path::Path::new("bin/x64"))
    );

    // Native default is unaffected.
    let picked = launch::select_launch_entry(&options, &app, false, false).unwrap();
    assert_eq!(picked.id, "3");
}

#[test]
fn launch_targets_lists_installed_builds_and_steam_modes() {
    use crate::launch::runners::wine_tkg::steam_mode::SteamMode;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("bin/x64")).unwrap();
    std::fs::write(tmp.path().join("bin/x64/factorio.exe"), "x").unwrap();
    std::fs::write(tmp.path().join("bin/x64/factorio"), "x").unwrap();

    let entry = |exe: &str, target: LaunchTarget| LaunchInfo {
        app_id: 427520,
        id: exe.to_string(),
        description: String::new(),
        executable: exe.to_string(),
        arguments: String::new(),
        workingdir: None,
        target,
    };
    let options = vec![
        entry("bin/x64/Factorio.exe", LaunchTarget::WindowsProton),
        entry("bin/Win32/Factorio.exe", LaunchTarget::WindowsProton),
        entry("bin/x64/factorio", LaunchTarget::NativeLinux),
    ];
    let app = LibraryGame {
        app_id: 427520,
        name: "Factorio".to_string(),
        install_path: Some(tmp.path().to_string_lossy().to_string()),
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
        platform: Some("linux".to_string()),
        from_windows_steam: false,
        size_on_disk: None,
    };

    let ids = |targets: &[LaunchTargetChoice]| targets.iter().map(|t| t.id).collect::<Vec<_>>();
    let default_id =
        |targets: &[LaunchTargetChoice]| targets.iter().find(|t| t.default).map(|t| t.id);
    let missing =
        |targets: &[LaunchTargetChoice]| targets.iter().map(|t| t.missing).collect::<Vec<_>>();

    // Everything available, native default.
    let targets = launch_targets(
        &options,
        &app,
        true,
        true,
        false,
        Some(LaunchTarget::NativeLinux),
        SteamMode::Standalone,
    );
    assert_eq!(
        ids(&targets),
        [
            "linux",
            "windows-runtime",
            "windows-host",
            "windows-standalone"
        ]
    );
    assert!(targets.iter().all(|t| t.available));
    assert_eq!(default_id(&targets), Some("linux"));
    assert_eq!(
        targets[2].flags,
        ["--platform", "windows", "--steam-mode", "host"]
    );

    // Missing Steam installs are flagged.
    let targets = launch_targets(
        &options,
        &app,
        false,
        false,
        true,
        Some(LaunchTarget::WindowsProton),
        SteamMode::Standalone,
    );
    assert_eq!(
        missing(&targets),
        [None, Some("steam-runtime"), Some("host-steam"), None]
    );
    assert_eq!(default_id(&targets), Some("windows-standalone"));
    assert_eq!(targets[0].flags, ["--platform", "linux", "--steam"]);

    // Windows default follows Steam mode.
    let targets = launch_targets(
        &options,
        &app,
        true,
        true,
        false,
        Some(LaunchTarget::WindowsProton),
        SteamMode::InWineRuntime,
    );
    assert_eq!(default_id(&targets), Some("windows-runtime"));

    // Missing build flags every target.
    std::fs::remove_file(tmp.path().join("bin/x64/factorio")).unwrap();
    let targets = launch_targets(
        &options,
        &app,
        false,
        true,
        false,
        None,
        SteamMode::Standalone,
    );
    assert_eq!(
        missing(&targets),
        [Some("build"), Some("steam-runtime"), None, None]
    );
    assert_eq!(default_id(&targets), Some("windows-host"));

    // Unoffered platforms aren't listed.
    let targets = launch_targets(
        &options[..2],
        &app,
        true,
        true,
        false,
        None,
        SteamMode::Standalone,
    );
    assert_eq!(
        ids(&targets),
        ["windows-runtime", "windows-host", "windows-standalone"]
    );
}
