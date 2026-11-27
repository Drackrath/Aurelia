use super::super::*;

fn entry(path: &str, kind: EntryKind, executable: bool, link: Option<&str>) -> ManifestFileEntry {
    ManifestFileEntry {
        filename: path.to_string(),
        kind,
        size: 2048,
        sha1: "ab".to_string(),
        chunks: 2,
        executable,
        link_target: link.map(str::to_string),
    }
}

#[test]
fn entry_cells_carry_markers() {
    let file = entry("bin/data.pak", EntryKind::File, false, None);
    assert_eq!(
        entry_cells(&file),
        [human_bytes(2048), "bin/data.pak".to_string()]
    );
    let exe = entry("bin/game", EntryKind::File, true, None);
    assert_eq!(entry_cells(&exe)[1], "bin/game*");
    let dir = entry("bin", EntryKind::Directory, false, None);
    assert_eq!(entry_cells(&dir), ["<dir>".to_string(), "bin/".to_string()]);
    let link = entry("lib.so", EntryKind::Symlink, false, Some("lib.so.1"));
    assert_eq!(
        entry_cells(&link),
        ["<link>".to_string(), "lib.so -> lib.so.1".to_string()]
    );
}

#[test]
fn files_json_adds_sha1_and_chunks() {
    let files = [
        entry("bin", EntryKind::Directory, false, None),
        entry("bin/game", EntryKind::File, true, None),
    ];
    let v = files_json(1, 2, 3, &files);
    assert_eq!(v["manifest_id"], 3);
    assert_eq!(v["total_size"], 2048);
    assert_eq!(v["files"][0]["kind"], "directory");
    assert_eq!(v["files"][1]["sha1"], "ab");
    assert_eq!(v["files"][1]["chunks"], 2);
    assert_eq!(v["files"][1]["executable"], true);
}
