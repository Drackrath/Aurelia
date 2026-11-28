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
    let app = LibraryGame {
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
    let linux = Some(DepotPlatform::Linux);
    let picked = launch::select_launch_entry(&options, &app, linux, false, false).unwrap();
    assert_eq!(picked.id, "1");

    // Explicit Proton/Windows request still picks the Windows entry.
    let picked = launch::select_launch_entry(&options, &app, linux, true, false).unwrap();
    assert_eq!(picked.id, "0");

    // Explicit native preference beats the windows manifest.
    let windows = Some(DepotPlatform::Windows);
    let picked = launch::select_launch_entry(&options, &app, windows, false, true).unwrap();
    assert_eq!(picked.id, "1");

    // Unknown installed platform: first existing executable.
    let picked = launch::select_launch_entry(&options, &app, None, false, false).unwrap();
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
    assert!(error.contains("doesn't run on this OS"), "{error}");
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
    assert!(error.contains("requires the native Linux build"), "{error}");

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
    let linux = Some(DepotPlatform::Linux);
    let picked = launch::select_launch_entry(&options, &app, linux, true, false).unwrap();
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
    let picked = launch::select_launch_entry(&options, &app, linux, false, false).unwrap();
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

#[test]
fn launch_targets_lists_an_installed_macos_build() {
    use crate::launch::runners::wine_tkg::steam_mode::SteamMode;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("Balatro.app/Contents/MacOS")).unwrap();
    let options = vec![launch_option("0", "Balatro.app", LaunchTarget::NativeMacos)];
    let app = LibraryGame {
        app_id: 2379780,
        name: "Balatro".to_string(),
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
        platform: None,
        from_windows_steam: false,
        size_on_disk: None,
    };
    let list = || {
        launch_targets(
            &options,
            &app,
            false,
            false,
            false,
            Some(LaunchTarget::NativeMacos),
            SteamMode::Standalone,
        )
    };

    assert_eq!(
        list(),
        [LaunchTargetChoice {
            id: "macos",
            platform: "macos",
            steam_mode: None,
            flags: Vec::new(),
            available: true,
            missing: None,
            default: true,
        }]
    );

    // A missing build is never the default.
    std::fs::remove_dir_all(tmp.path().join("Balatro.app")).unwrap();
    let targets = list();
    assert_eq!(targets[0].missing, Some("build"));
    assert!(!targets[0].default);
}

const DUAL_BUILD_APPINFO: &str = r#""appinfo"
{
	"appid"	"70"
	"config"
	{
		"launch"
		{
			"0" { "executable" "hl.exe" "config" { "oslist" "windows" } }
			"1" { "executable" "hl_linux" "config" { "oslist" "linux" } }
		}
	}
	"depots"
	{
		"71" { "config" { "oslist" "windows" } }
		"72" { "config" { "oslist" "linux" } }
		"73" { "maxsize" "1" }
		"74" { "config" { "oslist" "windows,linux" } }
		"75" { "config" { "oslist" "windows" } "dlcappid" "80" }
	}
}"#;

#[test]
fn launch_selection_follows_the_installed_depots() {
    let cases = [
        (
            "Linux install over a stale Windows exe",
            vec![(72, 2, 1), (73, 3, 1)],
            Some(DepotPlatform::Linux),
            "1",
        ),
        (
            "Windows install with a shared depot and a leftover Linux binary",
            vec![(71, 1, 1), (74, 4, 1)],
            Some(DepotPlatform::Windows),
            "0",
        ),
        (
            "Linux install with a carried-over Windows DLC depot",
            vec![(72, 2, 1), (75, 5, 1)],
            Some(DepotPlatform::Linux),
            "1",
        ),
        ("untagged depots only", vec![(73, 3, 1)], None, "0"),
    ];
    let client = SteamClient::new().unwrap();
    for (case, installed_depots, platform, id) in cases {
        let tmp = tempfile::tempdir().unwrap();
        let steamapps = tmp.path().join("steamapps");
        let install = steamapps.join("common").join("Half-Life");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(install.join("hl.exe"), "x").unwrap();
        std::fs::write(install.join("hl_linux"), "x").unwrap();
        let spec = AppManifestSpec {
            appid: 70,
            game_name: "Half-Life",
            installdir: "Half-Life",
            installed_depots,
            fully_installed: true,
            ..Default::default()
        };
        SteamClient::write_appmanifest(&steamapps.join("appmanifest_70.acf"), spec).unwrap();
        let app = LibraryGame {
            app_id: 70,
            name: "Half-Life".to_string(),
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
        };

        let (options, installed) = client
            .product_info_from_buffer(&app, DUAL_BUILD_APPINFO.as_bytes(), DepotPlatform::Linux)
            .unwrap();
        assert_eq!(installed, platform, "{case}");
        let picked = launch::select_launch_entry(&options, &app, installed, false, false).unwrap();
        assert_eq!(picked.id, id, "{case}");
    }
}

#[test]
fn explicit_windows_needs_a_windows_build() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("ShadowOfTheTombRaider.sh"), b"").unwrap();
    let mut app = LibraryGame {
        app_id: 750920,
        name: "Shadow of the Tomb Raider".to_string(),
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
        platform: Some("linux".to_string()),
        from_windows_steam: false,
        size_on_disk: None,
    };
    let options = vec![
        launch_option("0", "SOTTR.exe", LaunchTarget::WindowsProton),
        launch_option("1", "ShadowOfTheTombRaider.sh", LaunchTarget::NativeLinux),
    ];

    // Only Linux on disk: typed error.
    let err = launch::require_windows_build(&options, &app).unwrap_err();
    assert_eq!(
        crate::core::error::classify(&err).kind,
        crate::core::error::ErrorKind::BuildNotInstalled
    );
    assert_eq!(
        err.to_string(),
        "Windows build not installed; run aurelia install 750920 --platform windows"
    );

    // Windows exe present, any case.
    std::fs::write(tmp.path().join("sottr.exe"), b"").unwrap();
    assert!(launch::require_windows_build(&options, &app).is_ok());

    // Not installed counts as missing.
    app.is_installed = false;
    assert!(launch::require_windows_build(&options, &app).is_err());
}

#[test]
fn pinned_build_follows_the_manifest_over_leftovers() {
    use crate::core::models::DepotPlatform::{Linux, Windows};
    use launch::{PinnedBuild, pinned_build_state};
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("bin/x64")).unwrap();
    std::fs::write(tmp.path().join("bin/x64/factorio"), b"").unwrap();
    // Leftover from an older install.
    std::fs::write(tmp.path().join("bin/x64/factorio.exe"), b"").unwrap();
    let app = LibraryGame {
        app_id: 427520,
        name: "Factorio".to_string(),
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
        platform: Some("linux".to_string()),
        from_windows_steam: false,
        size_on_disk: None,
    };
    let options = vec![
        launch_option("0", "bin/x64/factorio.exe", LaunchTarget::WindowsProton),
        launch_option("1", "bin/x64/factorio", LaunchTarget::NativeLinux),
    ];
    let state = |installed, wanted| pinned_build_state(&options, &app, installed, wanted);

    // Linux depots: the exe is stale.
    assert_eq!(state(Some(Linux), Windows), PinnedBuild::Swapped);
    assert_eq!(state(Some(Linux), Linux), PinnedBuild::Ready);
    assert_eq!(state(Some(Windows), Linux), PinnedBuild::Swapped);
    assert_eq!(state(Some(Windows), Windows), PinnedBuild::Ready);
    // Unknown depots: trust the disk.
    assert_eq!(state(None, Windows), PinnedBuild::Ready);

    // Right depots, executable gone.
    std::fs::remove_file(tmp.path().join("bin/x64/factorio")).unwrap();
    assert_eq!(state(Some(Linux), Linux), PinnedBuild::Missing);
    assert_eq!(state(None, Linux), PinnedBuild::Missing);

    let err = launch::build_not_installed(427520, Linux);
    assert_eq!(
        err.to_string(),
        "Linux build not installed; run aurelia install 427520 --platform linux"
    );
    assert_eq!(
        crate::core::error::classify(&err).kind,
        crate::core::error::ErrorKind::BuildNotInstalled
    );
}
