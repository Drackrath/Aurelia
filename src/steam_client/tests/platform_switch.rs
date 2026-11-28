use super::super::*;
use crate::library::depot_browser::RawEntry;
use depot_files::ListedFile;
use platform_switch::reuse_old_build;
use sha1::Digest;

fn sha(data: &[u8]) -> Vec<u8> {
    sha1::Sha1::digest(data).to_vec()
}

fn file(path: &str, chunks: &[&[u8]]) -> ListedFile {
    let content: Vec<u8> = chunks.concat();
    let mut offset = 0;
    let chunks = chunks
        .iter()
        .map(|c| {
            let entry = (sha(c), offset, c.len() as u32);
            offset += c.len() as u64;
            entry
        })
        .collect();
    ListedFile {
        raw: RawEntry {
            path: path.to_string(),
            size: content.len() as u64,
            sha_content: sha(&content),
            ..Default::default()
        },
        chunks,
    }
}

#[test]
fn unchanged_file_moves_to_its_new_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("share/data")).unwrap();
    std::fs::write(dir.path().join("share/data/a.tiger"), b"aaaabbbb").unwrap();
    let old = [file("share/data/a.tiger", &[b"aaaa", b"bbbb"])];
    let new = [file("a.tiger", &[b"aaaa", b"bbbb"])];

    let missing = reuse_old_build(dir.path(), &old, &new);

    assert_eq!(missing, 0);
    assert_eq!(
        std::fs::read(dir.path().join("a.tiger")).unwrap(),
        b"aaaabbbb"
    );
    assert!(!dir.path().join("share/data/a.tiger").exists());
}

#[test]
fn changed_file_is_rebuilt_from_chunks_of_any_old_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("x.bin"), b"1111222233334444").unwrap();
    std::fs::write(dir.path().join("y.bin"), b"55556666").unwrap();
    let old = [
        file("x.bin", &[b"1111", b"2222", b"3333", b"4444"]),
        file("y.bin", &[b"5555", b"6666"]),
    ];
    let new = [file("z.bin", &[b"6666", b"9999", b"2222"])];

    let missing = reuse_old_build(dir.path(), &old, &new);

    assert_eq!(missing, 4);
    assert_eq!(
        std::fs::read(dir.path().join("z.bin")).unwrap(),
        b"6666\x00\x00\x00\x002222"
    );
    assert!(!dir.path().join("z.bin.aurelia-reuse").exists());
}

#[test]
fn rebuild_keeps_chunks_already_downloaded_at_the_target() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("old.bin"), b"aaaa").unwrap();
    std::fs::write(dir.path().join("new.bin"), b"\0\0\0\0cccc").unwrap();
    let old = [file("old.bin", &[b"aaaa"])];
    let new = [file("new.bin", &[b"aaaa", b"cccc"])];

    let missing = reuse_old_build(dir.path(), &old, &new);

    assert_eq!(missing, 0);
    assert_eq!(
        std::fs::read(dir.path().join("new.bin")).unwrap(),
        b"aaaacccc"
    );
}

#[test]
fn file_rewritten_in_place_reads_its_own_old_chunks() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("game.cfg"), b"keepdrop").unwrap();
    let old = [file("game.cfg", &[b"keep", b"drop"])];
    let new = [file("game.cfg", &[b"newc", b"keep"])];

    reuse_old_build(dir.path(), &old, &new);

    assert_eq!(
        std::fs::read(dir.path().join("game.cfg")).unwrap(),
        b"\0\0\0\0keep"
    );
}

#[test]
fn file_already_in_place_or_still_used_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("same.bin"), b"same").unwrap();
    std::fs::write(dir.path().join("short.bin"), b"x").unwrap();
    let old = [file("same.bin", &[b"same"]), file("short.bin", &[b"cccc"])];
    let new = [file("same.bin", &[b"same"]), file("other.bin", &[b"cccc"])];

    let missing = reuse_old_build(dir.path(), &old, &new);

    assert_eq!(missing, 4);
    assert!(dir.path().join("short.bin").exists());
    assert!(!dir.path().join("other.bin").exists());
}

#[test]
fn corrupt_old_chunk_is_not_copied() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("old.bin"), b"aaXa").unwrap();
    let old = [file("old.bin", &[b"aaaa"])];
    let new = [file("new.bin", &[b"aaaa", b"bbbb"])];

    let missing = reuse_old_build(dir.path(), &old, &new);

    assert_eq!(missing, 8);
    assert!(!dir.path().join("new.bin").exists());
    assert!(!dir.path().join("new.bin.aurelia-reuse").exists());
}

#[test]
fn interrupted_switch_counts_files_already_in_place() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("moved.bin"), b"aaaa").unwrap();
    std::fs::write(dir.path().join("fetched.bin"), b"bbbb\x00\x00\x00\x00").unwrap();
    let old = [file("share/moved.bin", &[b"aaaa"])];
    let new = [
        file("moved.bin", &[b"aaaa"]),
        file("fetched.bin", &[b"bbbb", b"cccc"]),
    ];

    let missing = reuse_old_build(dir.path(), &old, &new);

    assert_eq!(missing, 4);
}
