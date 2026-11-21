use super::super::*;

#[test]
fn splits_root_token() {
    assert_eq!(
        split_root_token("%WinAppDataLocalLow%SadSocket/9Kings/save.json"),
        Some(("WinAppDataLocalLow", "SadSocket/9Kings/save.json")),
    );
    // Classic remote-storage names have no token.
    assert_eq!(split_root_token("savegame.dat"), None);
}

#[test]
fn join_relative_is_bounded() {
    let base = Path::new("/base");
    // Leading slash, `.` and `..` components are ignored — no escaping the base.
    assert_eq!(
        join_relative(base, "/a/./b/../c"),
        Path::new("/base").join("a").join("b").join("c"),
    );
}

#[test]
fn classic_file_resolves_under_remote_root() {
    let resolver = CloudPathResolver::new(PathBuf::from("/r/remote"), None);
    assert_eq!(
        resolver.resolve("sub/save.dat").unwrap(),
        Path::new("/r/remote").join("sub").join("save.dat"),
    );
}

#[cfg(windows)]
#[test]
fn auto_cloud_token_maps_to_real_os_path() {
    // The exact case that was failing: the file landed in a phantom
    // `%WinAppDataLocalLow%SadSocket` folder instead of real LocalLow.
    let resolver = CloudPathResolver::new(PathBuf::from(r"C:\r\remote"), None);
    let resolved = resolver
        .resolve("%WinAppDataLocalLow%SadSocket/9Kings/9KingsSettings.json")
        .unwrap();
    let user = PathBuf::from(std::env::var_os("USERPROFILE").unwrap());
    assert_eq!(
        resolved,
        user.join("AppData")
            .join("LocalLow")
            .join("SadSocket")
            .join("9Kings")
            .join("9KingsSettings.json"),
    );
}

#[test]
fn glob_matches_handles_wildcards_case_insensitively() {
    assert!(glob_matches("*", "anything.dat"));
    assert!(glob_matches("", "anything.dat"));
    assert!(glob_matches("*.sav", "Game01.SAV"));
    assert!(glob_matches("save?.dat", "save7.dat"));
    assert!(!glob_matches("*.sav", "notes.txt"));
}

#[test]
fn discovers_local_saves_from_ufs_specs() {
    // %GameInstall% lets us point a UFS rule at a temp dir without touching real
    // user folders. A new save under it must be discovered and named correctly.
    let tmp = tempfile::tempdir().unwrap();
    let saves = tmp.path().join("saves").join("slot1");
    std::fs::create_dir_all(&saves).unwrap();
    std::fs::write(saves.join("game.sav"), b"data").unwrap();
    std::fs::write(saves.join("ignore.txt"), b"x").unwrap();

    let resolver = CloudPathResolver::new(PathBuf::from("/unused"), Some(tmp.path().to_path_buf()));
    let specs = [UfsSaveSpec {
        root: "GameInstall".to_string(),
        path: "saves".to_string(),
        pattern: "*.sav".to_string(),
        recursive: true,
    }];

    let found = discover_local_saves(&specs, &resolver);
    assert_eq!(found.len(), 1, "only the .sav should match");
    assert_eq!(found[0].0, "%GameInstall%saves/slot1/game.sav");
    assert_eq!(found[0].1, saves.join("game.sav"));
    // And the produced cloud name round-trips back to the same local path.
    assert_eq!(
        resolver.resolve(&found[0].0).unwrap(),
        saves.join("game.sav")
    );
}

fn linux_override() -> UfsRootOverride {
    UfsRootOverride {
        root: "WinAppDataRoaming".into(),
        os: "Linux".into(),
        os_compare: "=".into(),
        use_instead: "GameInstall".into(),
        add_path: "linuxsaves".into(),
        path_transforms: vec![("Company/".into(), "company/".into())],
    }
}

#[cfg(not(windows))]
#[test]
fn root_override_applies_only_to_its_os() {
    let overrides = [linux_override()];
    let linux = CloudPathResolver::new(PathBuf::from("/r"), Some(PathBuf::from("/game")))
        .with_wine_prefix(Some(PathBuf::from("/pfx")))
        .with_root_overrides(&overrides, "linux");
    assert_eq!(
        linux
            .resolve("%WinAppDataRoaming%Company/Game/s.dat")
            .unwrap(),
        Path::new("/game/linuxsaves/company/Game/s.dat")
    );

    let windows = CloudPathResolver::new(PathBuf::from("/r"), Some(PathBuf::from("/game")))
        .with_wine_prefix(Some(PathBuf::from("/pfx")))
        .with_root_overrides(&overrides, "windows");
    assert_eq!(
        windows
            .resolve("%WinAppDataRoaming%Company/Game/s.dat")
            .unwrap(),
        Path::new("/pfx/drive_c/users/steamuser/AppData/Roaming/Company/Game/s.dat")
    );

    // `!=` inverts the match.
    let mut not_linux = linux_override();
    not_linux.os_compare = "!=".into();
    assert!(!not_linux.applies_to("Linux"));
    assert!(not_linux.applies_to("windows"));
}

#[test]
fn steam_id_placeholders_substituted() {
    let r = CloudPathResolver::new(PathBuf::from("/r"), None).with_steam_id(76561197960287930);
    assert_eq!(
        r.substitute_ids("saves/{64BitSteamID}/{Steam3AccountID}"),
        "saves/76561197960287930/22202"
    );
    // Without an id: unchanged.
    let bare = CloudPathResolver::new(PathBuf::from("/r"), None);
    assert_eq!(bare.substitute_ids("{64BitSteamID}"), "{64BitSteamID}");
}

#[test]
fn discovery_finds_saves_under_substituted_id_and_override() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("linuxsaves/company/Game/76561197960287930");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("s.dat"), b"x").unwrap();

    let resolver = CloudPathResolver::new(PathBuf::from("/r"), Some(tmp.path().to_path_buf()))
        .with_root_overrides(&[linux_override()], "linux")
        .with_steam_id(76561197960287930);
    let specs = [UfsSaveSpec {
        root: "WinAppDataRoaming".into(),
        path: "Company/Game/{64BitSteamID}".into(),
        pattern: "*.dat".into(),
        recursive: false,
    }];
    let found = discover_local_saves(&specs, &resolver);
    assert_eq!(found.len(), 1);
    // Cloud name keeps Steam's Windows form.
    assert_eq!(
        found[0].0,
        "%WinAppDataRoaming%Company/Game/76561197960287930/s.dat"
    );
    assert_eq!(resolver.resolve(&found[0].0).unwrap(), dir.join("s.dat"));
}

#[test]
fn non_recursive_spec_skips_subdirectories() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("top.sav"), b"a").unwrap();
    let sub = tmp.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("deep.sav"), b"b").unwrap();

    let resolver = CloudPathResolver::new(PathBuf::from("/unused"), Some(tmp.path().to_path_buf()));
    let specs = [UfsSaveSpec {
        root: "GameInstall".to_string(),
        path: String::new(),
        pattern: "*".to_string(),
        recursive: false,
    }];

    let names: Vec<_> = discover_local_saves(&specs, &resolver)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(names, vec!["%GameInstall%top.sav".to_string()]);
}

#[cfg(windows)]
#[test]
fn game_install_token_needs_install_dir() {
    let with_dir =
        CloudPathResolver::new(PathBuf::from(r"C:\r"), Some(PathBuf::from(r"C:\games\foo")));
    assert_eq!(
        with_dir.resolve("%GameInstall%saves/a.sav").unwrap(),
        Path::new(r"C:\games\foo").join("saves").join("a.sav"),
    );
    // Without an install dir the token can't be resolved.
    let without = CloudPathResolver::new(PathBuf::from(r"C:\r"), None);
    assert!(without.resolve("%GameInstall%saves/a.sav").is_err());
}

#[cfg(not(windows))]
#[test]
fn win_token_maps_into_wine_prefix() {
    // The failing case for Proton games: every cloud filename carries a %Win*%
    // root, which has no counterpart in the Linux home — it only exists inside
    // the game's wine prefix.
    let resolver = CloudPathResolver::new(PathBuf::from("/r/remote"), None)
        .with_wine_prefix(Some(PathBuf::from("/steam/compatdata/1903340/pfx")));

    let steamuser = Path::new("/steam/compatdata/1903340/pfx/drive_c/users/steamuser");
    assert_eq!(
        resolver
            .resolve("%WinAppDataLocal%Sandfall/Saved/SaveGames/EXPEDITION_0.sav")
            .unwrap(),
        steamuser
            .join("AppData/Local")
            .join("Sandfall/Saved/SaveGames/EXPEDITION_0.sav"),
    );
    assert_eq!(
        resolver.resolve("%WinMyDocuments%My Game/a.sav").unwrap(),
        steamuser.join("Documents").join("My Game").join("a.sav"),
    );
    assert_eq!(
        resolver.resolve("%WinSavedGames%b.sav").unwrap(),
        steamuser.join("Saved Games").join("b.sav"),
    );
    // Linux-native roots keep working and ignore the prefix entirely.
    assert!(
        resolver
            .resolve("%LinuxHome%.local/share/x.sav")
            .unwrap()
            .starts_with(std::env::var("HOME").unwrap())
    );
}

#[cfg(not(windows))]
#[test]
fn win_token_without_prefix_is_still_an_error() {
    // No prefix known — the file genuinely can't be placed. It must fail loudly
    // rather than resolve to some plausible-looking path in the Linux home.
    let resolver = CloudPathResolver::new(PathBuf::from("/r/remote"), None);
    assert!(resolver.resolve("%WinAppDataLocal%a/b.sav").is_err());
}
