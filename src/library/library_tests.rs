use super::*;

#[test]
fn ignores_tooling_by_app_id() {
    assert!(is_ignored_steam_app(228980, "Steamworks Common Redistributables"));
    assert!(is_ignored_steam_app(1628350, "")); // Steam Linux Runtime 3.0
    assert!(is_ignored_steam_app(1493710, "Proton Experimental"));
}

#[test]
fn ignores_tooling_by_name_prefix() {
    // App id not in the list, but the name marks it as tooling.
    assert!(is_ignored_steam_app(9999999, "Proton 9.0 (Beta)"));
    assert!(is_ignored_steam_app(9999998, "  Steam Linux Runtime 4.0"));
}

#[test]
fn keeps_real_games() {
    assert!(!is_ignored_steam_app(620, "Portal 2"));
    // A game that merely contains "Proton" mid-name is not tooling.
    assert!(!is_ignored_steam_app(12345, "The Protonist"));
}

#[test]
fn build_game_library_filters_tooling() {
    let owned = vec![
        OwnedGame {
            app_id: 620,
            name: "Portal 2".to_string(),
            playtime_forever_minutes: 0,
            local_manifest_ids: HashMap::new(),
            update_available: false,
        },
        OwnedGame {
            app_id: 228980,
            name: "Steamworks Common Redistributables".to_string(),
            playtime_forever_minutes: 0,
            local_manifest_ids: HashMap::new(),
            update_available: false,
        },
    ];
    let lib = build_game_library(owned, HashMap::new(), None);
    assert_eq!(lib.games.len(), 1);
    assert_eq!(lib.games[0].app_id, 620);
}

/// The modern (post-2021) shape, as Steam actually writes it: nested blocks with
/// an `apps` sub-block of appid -> byte size. The previous serde-derived parser
/// returned *nothing* here, so only the root library was ever scanned.
#[test]
fn parses_modern_library_folders_with_apps_blocks() {
    let vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"/home/user/.local/share/Steam"
		"label"		""
		"totalsize"		"0"
		"apps"
		{
			"228980"		"157818239"
			"440"		"53070236837"
		}
	}
	"1"
	{
		"path"		"/media/user/external/SteamLibrary"
		"label"		""
		"totalsize"		"1000186314752"
		"apps"
		{
			"620"		"47059987075"
		}
	}
}
"#;
    // Both libraries, and no appid from an `apps` block mistaken for a path.
    assert_eq!(
        library_folder_paths(vdf),
        vec![
            PathBuf::from("/home/user/.local/share/Steam"),
            PathBuf::from("/media/user/external/SteamLibrary"),
        ]
    );
}

/// The legacy pre-2021 shape, where a numbered key maps straight to a path.
/// Non-numeric bookkeeping keys at the same level are not libraries.
#[test]
fn parses_legacy_flat_library_folders() {
    let vdf = "\"LibraryFolders\"\n{\n\t\"TimeNextStatsReport\"\t\t\"1500000000\"\n\t\"ContentStatsID\"\t\t\"42\"\n\t\"1\"\t\t\"/mnt/games/SteamLibrary\"\n}\n";
    assert_eq!(
        library_folder_paths(vdf),
        vec![PathBuf::from("/mnt/games/SteamLibrary")]
    );
}

#[test]
fn unescapes_windows_library_paths() {
    let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
    assert_eq!(
        library_folder_paths(vdf),
        vec![PathBuf::from(r"D:\SteamLibrary")]
    );
}

#[test]
fn skips_empty_and_malformed_entries() {
    let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"\"\n\t}\n\t\"label\"\n\t{\n\t\t\"path\"\t\t\"/not/a/library\"\n\t}\n}\n";
    assert!(library_folder_paths(vdf).is_empty());
}

#[cfg(not(target_os = "windows"))]
#[test]
fn decodes_octal_escapes_in_mount_points() {
    assert_eq!(unescape_mount_target("/media/user/disk"), "/media/user/disk");
    assert_eq!(
        unescape_mount_target("/media/user/My\\040Disk"),
        "/media/user/My Disk"
    );
    // Multi-byte names survive: the escape decodes to a byte, not a char.
    assert_eq!(
        unescape_mount_target("/media/user/Spie\\040lé"),
        "/media/user/Spie lé"
    );
}

/// The probe must find a library by its `steamapps/` directory whatever the
/// folder is called, at the volume root or nested below it — and must not report
/// directories that merely sit nearby.
#[test]
fn probes_for_libraries_by_structure_not_by_name() {
    let temp = std::env::temp_dir().join(format!("aurelia-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);

    // A deliberately unguessable name, one nested a level down, one too deep to
    // reach, plus a decoy with no `steamapps` at all.
    let flat = temp.join("Spiele-Sammlung");
    let nested = temp.join("Games").join("zweite-platte");
    let too_deep = temp.join("a").join("b").join("c");
    for library in [&flat, &nested, &too_deep] {
        std::fs::create_dir_all(library.join("steamapps")).unwrap();
    }
    std::fs::create_dir_all(temp.join("Documents")).unwrap();
    // Hidden directories are skipped even when they look like a library.
    std::fs::create_dir_all(temp.join(".cache").join("steamapps")).unwrap();

    let mut found = Vec::new();
    probe_for_libraries(&temp, MAX_PROBE_DEPTH, &mut found);
    found.sort();

    assert_eq!(found, vec![nested, flat]);

    std::fs::remove_dir_all(&temp).unwrap();
}

/// A library is never nested inside another, so the probe stops at the first hit
/// instead of walking a library's own `common/` tree.
#[test]
fn probe_does_not_descend_into_a_found_library() {
    let temp = std::env::temp_dir().join(format!("aurelia-probe-nested-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);

    let library = temp.join("SomeLibrary");
    std::fs::create_dir_all(library.join("steamapps").join("common")).unwrap();
    std::fs::create_dir_all(library.join("inner").join("steamapps")).unwrap();

    let mut found = Vec::new();
    probe_for_libraries(&temp, MAX_PROBE_DEPTH, &mut found);

    assert_eq!(found, vec![library]);

    std::fs::remove_dir_all(&temp).unwrap();
}

fn copy_at(path: &str, last_updated: u64, build_id: u64) -> InstalledAppInfo {
    InstalledAppInfo {
        install_path: PathBuf::from(path),
        active_branch: "public".to_string(),
        name: Some("Game".to_string()),
        last_owner: None,
        from_windows_steam: false,
        manifest_path: PathBuf::from(path).join("appmanifest.acf"),
        last_updated,
        build_id,
    }
}

#[test]
fn live_copy_is_the_most_recently_updated() {
    // The bug this replaces: whichever library sorted last won, so a stale copy
    // on an external drive shadowed a fresh install and the game reported an
    // available update forever.
    let stale = copy_at("/run/media/vol/SteamLibrary/common/G", 1_781_182_506, 23_651_528);
    let fresh = copy_at("/home/u/.local/share/Steam/common/G", 1_786_492_022, 24_486_608);

    // Stable whichever order they are scanned in.
    for copies in [
        vec![stale.clone(), fresh.clone()],
        vec![fresh.clone(), stale.clone()],
    ] {
        let live = pick_live_copy(copies).unwrap();
        assert_eq!(live.install_path, fresh.install_path);
    }
}

#[test]
fn build_id_breaks_a_last_updated_tie() {
    let older = copy_at("/a", 1_786_492_022, 23_000_000);
    let newer = copy_at("/b", 1_786_492_022, 24_486_608);
    assert_eq!(pick_live_copy(vec![older, newer]).unwrap().install_path, PathBuf::from("/b"));
}

#[test]
fn a_single_copy_is_returned_unchanged() {
    let only = copy_at("/only", 0, 0);
    assert_eq!(pick_live_copy(vec![only]).unwrap().install_path, PathBuf::from("/only"));
    assert!(pick_live_copy(vec![]).is_none());
}

#[tokio::test]
async fn symlinked_steam_roots_collapse_to_one_library() {
    // A standard install has ~/.steam/root and ~/.steam/steam both symlinked to
    // ~/.local/share/Steam. Deduping by path string alone leaves the same library
    // listed three times, so every game in it looks installed three times over.
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("local/share/Steam");
    std::fs::create_dir_all(real.join("steamapps")).unwrap();
    let link_root = tmp.path().join("root");
    let link_steam = tmp.path().join("steam");
    std::os::unix::fs::symlink(&real, &link_root).unwrap();
    std::os::unix::fs::symlink(&real, &link_steam).unwrap();

    let libraries =
        expand_library_roots(&[link_root, link_steam, real.clone()]).await;

    let canonical_real = std::fs::canonicalize(&real).unwrap();
    assert_eq!(
        libraries.iter().filter(|p| **p == canonical_real).count(),
        1,
        "the same library must appear once, got {libraries:?}"
    );
}
