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

#[tokio::test]
async fn parse_library_folders_skips_odd_entries_keeps_good_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let vdf = tmp.path().join("libraryfolders.vdf");
    // Entry 0: fine. Entry 1: legacy bare-string path. Entry 2: block with no
    // "path" key at all. "contentstatsid": non-numeric top-level key.
    std::fs::write(
        &vdf,
        "\"libraryfolders\"\n{\n\t\"contentstatsid\"\t\t\"-42\"\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/good/library\"\n\t}\n\t\"1\"\t\t\"/legacy/library\"\n\t\"2\"\n\t{\n\t\t\"label\"\t\t\"no path here\"\n\t}\n}\n",
    )
    .unwrap();

    let found = parse_library_folders(vdf).await.expect("odd entries must not fail the parse");
    assert!(found.contains(&PathBuf::from("/good/library")));
    assert!(found.contains(&PathBuf::from("/legacy/library")));
    assert_eq!(found.len(), 2, "the path-less entry is skipped, not fatal");
}

#[tokio::test]
async fn parse_library_folders_reads_real_world_layout() {
    // Regression: keyvalues_serde consumes the root "libraryfolders" key itself,
    // so parsing into a struct with a `libraryfolders` field silently matched
    // nothing and every real file yielded an empty list.
    let tmp = tempfile::tempdir().unwrap();
    let vdf = tmp.path().join("libraryfolders.vdf");
    std::fs::write(
        &vdf,
        "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/good/library\"\n\t\t\"label\"\t\t\"\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"620\"\t\t\"1\"\n\t\t}\n\t}\n}\n",
    )
    .unwrap();
    let found = parse_library_folders(vdf).await.unwrap();
    assert_eq!(found, vec![PathBuf::from("/good/library")]);
}

#[tokio::test]
async fn malformed_libraryfolders_does_not_abort_scan() {
    let tmp = tempfile::tempdir().unwrap();
    let steamapps = tmp.path().join("steamapps");
    std::fs::create_dir_all(&steamapps).unwrap();
    // Syntactically broken VDF (unbalanced braces / truncated).
    std::fs::write(steamapps.join("libraryfolders.vdf"), "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path").unwrap();
    // A valid manifest in the same library must still be found.
    std::fs::write(
        steamapps.join("appmanifest_620.acf"),
        "\"AppState\"\n{\n\t\"appid\"\t\t\"620\"\n\t\"name\"\t\t\"Portal 2\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"installdir\"\t\t\"Portal 2\"\n}\n",
    )
    .unwrap();

    let installed = scan_library_info(tmp.path())
        .await
        .expect("a malformed libraryfolders.vdf must not abort the scan");
    let info = installed.get(&620).expect("game in the scanned root must still be found");
    assert_eq!(info.name.as_deref(), Some("Portal 2"));
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
