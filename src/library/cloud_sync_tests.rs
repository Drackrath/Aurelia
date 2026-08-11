use super::*;

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

fn cloud(hash: &str, ts: u64) -> CloudFileEntry {
    CloudFileEntry {
        filename: "save.dat".to_string(),
        timestamp: ts,
        size: 10,
        sha_hash: Some(hash.to_string()),
    }
}
fn local(hash: &str, ts: u64) -> LocalInfo {
    LocalInfo { hash: hash.to_string(), size: 10, timestamp: ts }
}
fn base(hash: &str) -> BaselineEntry {
    BaselineEntry { hash: hash.to_string(), timestamp: 0, size: 10 }
}

#[test]
fn identical_content_is_skipped_regardless_of_timestamp() {
    // Same bytes on both sides but different mtimes must NOT cause a transfer.
    let action = plan_action(None, Some(&local("aaaa", 200)), Some(&cloud("AAAA", 100)));
    assert_eq!(action, PlannedAction::Skip, "hash match wins over timestamps");
}

#[test]
fn only_local_changed_uploads_not_conflicts() {
    // The normal play loop: baseline==cloud, the user played so local advanced.
    // This must be a plain upload, never a conflict prompt.
    let action = plan_action(Some(&base("cccc")), Some(&local("llll", 300)), Some(&cloud("cccc", 100)));
    assert_eq!(action, PlannedAction::Upload);
}

#[test]
fn only_cloud_changed_downloads() {
    // Played on another machine: baseline==local, cloud advanced. Plain download.
    let action = plan_action(Some(&base("llll")), Some(&local("llll", 100)), Some(&cloud("cccc", 300)));
    assert_eq!(action, PlannedAction::Download);
}

#[test]
fn both_changed_is_a_conflict() {
    // Both sides diverged from the last-synced baseline — the data-loss case.
    let action = plan_action(Some(&base("oldd")), Some(&local("llll", 300)), Some(&cloud("cccc", 290)));
    assert_eq!(action, PlannedAction::Conflict);
}

#[test]
fn first_sync_with_two_differing_copies_is_a_conflict() {
    // No baseline yet and the copies differ: we can't know which lineage wins.
    let action = plan_action(None, Some(&local("llll", 300)), Some(&cloud("cccc", 100)));
    assert_eq!(action, PlannedAction::Conflict);
}

#[test]
fn one_sided_presence_moves_the_only_way_it_can() {
    assert_eq!(plan_action(None, Some(&local("llll", 1)), None), PlannedAction::Upload);
    assert_eq!(plan_action(None, None, Some(&cloud("cccc", 1))), PlannedAction::Download);
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

    let resolver =
        CloudPathResolver::new(PathBuf::from("/unused"), Some(tmp.path().to_path_buf()));
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
    assert_eq!(resolver.resolve(&found[0].0).unwrap(), saves.join("game.sav"));
}

#[test]
fn non_recursive_spec_skips_subdirectories() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("top.sav"), b"a").unwrap();
    let sub = tmp.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("deep.sav"), b"b").unwrap();

    let resolver =
        CloudPathResolver::new(PathBuf::from("/unused"), Some(tmp.path().to_path_buf()));
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
    assert!(resolver
        .resolve("%LinuxHome%.local/share/x.sav")
        .unwrap()
        .starts_with(std::env::var("HOME").unwrap()));
}

#[cfg(not(windows))]
#[test]
fn win_token_without_prefix_is_still_an_error() {
    // No prefix known — the file genuinely can't be placed. It must fail loudly
    // rather than resolve to some plausible-looking path in the Linux home.
    let resolver = CloudPathResolver::new(PathBuf::from("/r/remote"), None);
    assert!(resolver.resolve("%WinAppDataLocal%a/b.sav").is_err());
}

#[test]
fn skipped_files_are_reported_not_dropped() {
    // An unresolvable file must be visible in the outcome; a bare "0 downloaded"
    // is indistinguishable from an empty cloud.
    let outcome = SyncOutcome {
        skipped: vec![
            SyncSkip {
                filename: "%WinAppDataLocal%a.sav".to_string(),
                root_token: Some("WinAppDataLocal".to_string()),
                reason: "unsupported".to_string(),
            },
            SyncSkip {
                filename: "%WinAppDataLocal%b.sav".to_string(),
                root_token: Some("WinAppDataLocal".to_string()),
                reason: "unsupported".to_string(),
            },
            SyncSkip {
                filename: "%FutureRoot%c.sav".to_string(),
                root_token: Some("FutureRoot".to_string()),
                reason: "unsupported".to_string(),
            },
        ],
        ..Default::default()
    };
    assert!(outcome.has_skips());
    assert!(!outcome.has_conflicts());
    // Deduplicated: two files sharing a root are one problem, not two.
    assert_eq!(
        outcome.skipped_tokens(),
        vec!["FutureRoot".to_string(), "WinAppDataLocal".to_string()],
    );
}

/// Build a Steam-style compressed cloud body: a zip holding the single real file.
fn zip_wrap(content: &[u8]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        w.start_file::<_, ()>("z", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut w, content).unwrap();
        w.finish().unwrap();
    }
    buf.into_inner()
}

fn sha1_hex(bytes: &[u8]) -> String {
    hex_lower(&Sha1::digest(bytes))
}

#[test]
fn compressed_download_is_unwrapped_to_the_real_file() {
    // The failing case: Steam ships the save inside a zip, so writing the body
    // verbatim leaves a zip on disk where the game expects its save. The reported
    // hash is over the *original* content, which is what identifies the unwrap.
    let content = b"GVAS real save payload".repeat(64);
    let body = zip_wrap(&content);
    assert_ne!(body, content, "test fixture must actually be wrapped");

    let out = maybe_decompress(
        body,
        29_112,
        content.len() as u32,
        Some(&sha1_hex(&content)),
        "%WinAppDataLocal%EXPEDITION_0.sav",
    );
    assert_eq!(out, content);
}

#[test]
fn uncompressed_download_passes_through_untouched() {
    let content = b"plain save bytes".to_vec();
    let out = maybe_decompress(
        content.clone(),
        content.len() as u32,
        content.len() as u32,
        Some(&sha1_hex(&content)),
        "save.dat",
    );
    assert_eq!(out, content);
}

#[test]
fn a_save_that_is_itself_a_zip_is_not_unwrapped() {
    // Some games store saves as zip archives. Steam's hash identifies the file as
    // already-correct, so it must be written as received rather than unpacked.
    let content = zip_wrap(b"inner game data");
    let out = maybe_decompress(
        content.clone(),
        content.len() as u32,
        content.len() as u32,
        Some(&sha1_hex(&content)),
        "archive.sav",
    );
    assert_eq!(out, content, "a genuine zip save must survive intact");
}

#[test]
fn decompresses_on_size_mismatch_when_no_hash_is_known() {
    let content = b"payload without a reported hash".repeat(32);
    let body = zip_wrap(&content);
    let out = maybe_decompress(body, 100, content.len() as u32, None, "save.dat");
    assert_eq!(out, content);
}

#[test]
fn a_failed_transfer_is_recorded_and_marks_the_run_incomplete() {
    // The case that left a save set half-restored: one file failing must not be
    // readable as success, because the game will start on the partial set.
    let mut outcome = SyncOutcome {
        downloaded: vec!["a.sav".to_string()],
        ..Default::default()
    };
    outcome.record_failure(
        "%WinAppDataLocal%EXPEDITION_0.sav",
        "down",
        &anyhow::anyhow!("Response timed out"),
    );

    assert!(outcome.has_failures());
    assert!(outcome.is_incomplete());
    assert_eq!(outcome.failed[0].direction, "down");
    assert!(outcome.failed[0].error.contains("timed out"));
    // The successful transfer is still reported — partial progress is kept.
    assert_eq!(outcome.downloaded, vec!["a.sav".to_string()]);
}

#[test]
fn a_clean_run_is_not_incomplete() {
    let outcome = SyncOutcome {
        downloaded: vec!["a.sav".to_string()],
        ..Default::default()
    };
    assert!(!outcome.is_incomplete());
    assert!(!outcome.has_failures());
}
