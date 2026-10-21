use super::resolve_path_ignore_case;

fn touch(path: &std::path::Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "x").unwrap();
}

#[test]
fn exact_path_resolves_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    touch(&tmp.path().join("bin/x64/game.exe"));
    let got = resolve_path_ignore_case(tmp.path(), "bin/x64/game.exe").unwrap();
    assert_eq!(got, tmp.path().join("bin/x64/game.exe"));
}

#[test]
fn mismatched_case_finds_on_disk_name() {
    let tmp = tempfile::tempdir().unwrap();
    touch(&tmp.path().join("bin/x64/factorio.exe"));
    let got = resolve_path_ignore_case(tmp.path(), "BIN/x64/Factorio.exe").unwrap();
    assert_eq!(got, tmp.path().join("bin/x64/factorio.exe"));
}

#[test]
fn backslash_separators_are_split() {
    let tmp = tempfile::tempdir().unwrap();
    touch(&tmp.path().join("Binaries/Win64/Game.exe"));
    let got = resolve_path_ignore_case(tmp.path(), "binaries\\win64\\game.exe").unwrap();
    assert_eq!(got, tmp.path().join("Binaries/Win64/Game.exe"));
}

#[test]
fn exact_match_beats_case_variant() {
    let tmp = tempfile::tempdir().unwrap();
    touch(&tmp.path().join("Game.exe"));
    touch(&tmp.path().join("game.exe"));
    // A case-insensitive filesystem (NTFS, exFAT) keeps one file, so there is no
    // variant to beat.
    if std::fs::read_dir(tmp.path()).unwrap().count() < 2 {
        return;
    }
    let got = resolve_path_ignore_case(tmp.path(), "game.exe").unwrap();
    assert_eq!(got, tmp.path().join("game.exe"));
}

#[test]
fn missing_component_is_none() {
    let tmp = tempfile::tempdir().unwrap();
    touch(&tmp.path().join("bin/game.exe"));
    assert!(resolve_path_ignore_case(tmp.path(), "bin/other.exe").is_none());
    assert!(resolve_path_ignore_case(tmp.path(), "nope/game.exe").is_none());
}
