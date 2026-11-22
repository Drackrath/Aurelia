use super::super::*;
use crate::steam_client::DepotManifestInfo;

fn raw(path: &str, flags: u32, link: &str) -> RawEntry {
    RawEntry {
        path: path.to_string(),
        flags,
        link_target: link.to_string(),
        size: 3,
        sha_content: vec![0xab, 0xcd],
        chunks: 1,
    }
}

#[test]
fn entry_kind_from_flags() {
    assert_eq!(entry_kind(0, ""), EntryKind::File);
    assert_eq!(entry_kind(FLAG_EXECUTABLE, ""), EntryKind::File);
    assert_eq!(entry_kind(FLAG_DIRECTORY, ""), EntryKind::Directory);
    assert_eq!(entry_kind(FLAG_SYMLINK, ""), EntryKind::Symlink);
    assert_eq!(entry_kind(0, "lib.so.1"), EntryKind::Symlink);
}

#[test]
fn entries_sort_skip_root_and_mark() {
    let rows = entries([
        raw("bin/game", FLAG_EXECUTABLE, ""),
        raw("", FLAG_DIRECTORY, ""),
        raw("bin", FLAG_DIRECTORY, ""),
        raw("bin/lib.so", 0, "lib\\real.so"),
    ]);
    let paths: Vec<_> = rows.iter().map(|r| r.filename.as_str()).collect();
    assert_eq!(paths, ["bin", "bin/game", "bin/lib.so"]);
    assert_eq!(rows[0].kind, EntryKind::Directory);
    assert!(rows[0].sha1.is_empty());
    assert!(rows[1].executable);
    assert_eq!(rows[1].sha1, "abcd");
    assert_eq!(rows[2].kind, EntryKind::Symlink);
    assert_eq!(rows[2].link_target.as_deref(), Some("lib/real.so"));
    assert_eq!(total_size(&rows), 3);
}

#[test]
fn glob_matching() {
    assert!(glob_match("*.exe", "bin/Game.EXE"));
    assert!(glob_match("bin/*", "bin/x/y.dll"));
    assert!(glob_match("b?n/*.dll", "bin/a.dll"));
    assert!(glob_match("*", ""));
    assert!(!glob_match("*.exe", "bin/game.dll"));
    assert!(!glob_match("bin/?", "bin/ab"));
    assert!(glob_match("*a*b*", "xxaxxbxx"));
}

#[test]
fn normalize_request_strips_leading_separators() {
    assert_eq!(normalize_request("bin/game.exe"), "bin/game.exe");
    assert_eq!(normalize_request("/bin/game.exe"), "bin/game.exe");
    assert_eq!(normalize_request(" .//bin\\game.exe "), "bin/game.exe");
}

#[test]
fn only_regular_files_download() {
    assert!(ensure_file("a", EntryKind::File).is_ok());
    assert!(ensure_file("a", EntryKind::Directory).is_err());
    assert!(ensure_file("a", EntryKind::Symlink).is_err());
}

fn info(depot_id: u32, branch: &str, manifest_id: u64) -> DepotManifestInfo {
    DepotManifestInfo {
        depot_id,
        depot_name: None,
        branch: branch.to_string(),
        manifest_id,
        size: 0,
    }
}

#[test]
fn manifest_ref_picks_id_branch_or_public() {
    assert_eq!(
        ManifestRef::from_options(Some(7), Some("beta")),
        ManifestRef::Id(7)
    );
    assert_eq!(
        ManifestRef::from_options(None, Some("beta")),
        ManifestRef::Branch("beta".into())
    );
    assert_eq!(
        ManifestRef::from_options(None, None),
        ManifestRef::default()
    );

    let list = [
        info(1, "public", 10),
        info(1, "beta", 11),
        info(2, "public", 20),
    ];
    assert_eq!(
        pick_manifest(&list, 1, &ManifestRef::default()).unwrap(),
        10
    );
    assert_eq!(
        pick_manifest(&list, 1, &ManifestRef::Branch("BETA".into())).unwrap(),
        11
    );
    assert_eq!(pick_manifest(&list, 9, &ManifestRef::Id(5)).unwrap(), 5);
    let err = pick_manifest(&list, 2, &ManifestRef::Branch("beta".into())).unwrap_err();
    assert!(err.to_string().contains("branches: public"), "{err}");
    assert!(pick_manifest(&list, 3, &ManifestRef::default()).is_err());
}
