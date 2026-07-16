use super::*;
use std::path::Path;

const SAMPLE: &str = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"label\"\t\t\"\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"228980\"\t\t\"123456\"\n\t\t\t\"620\"\t\t\"789012\"\n\t\t}\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t\t\"label\"\t\t\"\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"440\"\t\t\"111\"\n\t\t}\n\t}\n}\n";

#[test]
fn moves_entry_between_apps_blocks() {
    let out = update_libraryfolders_apps(
        SAMPLE,
        620,
        Path::new("C:\\Program Files (x86)\\Steam"),
        Path::new("D:\\SteamLibrary"),
        789012,
    )
    .expect("should rewrite");

    // Removed from source block.
    let zero_block = &out[..out.find("\"1\"").unwrap()];
    assert!(!zero_block.contains("\"620\""), "620 should be gone from folder 0");
    // Other source entries are untouched.
    assert!(zero_block.contains("\"228980\""));
    // Added to destination block.
    let one_block = &out[out.find("\"1\"").unwrap()..];
    assert!(one_block.contains("\"620\"\t\t\"789012\""), "620 should be in folder 1");
    assert!(one_block.contains("\"440\""));
}

#[test]
fn destination_missing_returns_none() {
    // Destination path not present in the file → don't touch it.
    let out = update_libraryfolders_apps(
        SAMPLE,
        620,
        Path::new("C:\\Program Files (x86)\\Steam"),
        Path::new("E:\\Nope"),
        1,
    );
    assert!(out.is_none());
}

#[test]
fn missing_source_entry_still_adds_to_destination() {
    // App not currently indexed anywhere, but destination exists → add it.
    let out = update_libraryfolders_apps(
        SAMPLE,
        999,
        Path::new("C:\\Program Files (x86)\\Steam"),
        Path::new("D:\\SteamLibrary"),
        42,
    )
    .expect("should add to destination");
    let one_block = &out[out.find("\"1\"").unwrap()..];
    assert!(one_block.contains("\"999\"\t\t\"42\""));
}

#[test]
fn move_dir_copies_all_files() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    std::fs::write(src.join("a.bin"), vec![1u8; 100]).unwrap();
    std::fs::write(src.join("sub/b.bin"), vec![2u8; 200]).unwrap();

    let total = dir_size(&src);
    assert_eq!(total, 300);

    let mut last = 0u64;
    move_dir_with_progress(&src, &dst, total, |copied, _| last = copied).unwrap();

    assert!(!src.exists(), "source should be removed after move");
    assert_eq!(std::fs::read(dst.join("a.bin")).unwrap().len(), 100);
    assert_eq!(std::fs::read(dst.join("sub/b.bin")).unwrap().len(), 200);
    assert_eq!(last, total);
}
