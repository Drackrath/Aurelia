use super::super::*;

#[test]
fn soundtrack_resolves_to_the_music_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let steamapps = tmp.path().join("steamapps");
    std::fs::create_dir_all(steamapps.join("music/Lords of the Fallen - Soundtrack")).unwrap();
    let dir = resolve_install_dir(&steamapps, "Lords of the Fallen - Soundtrack");
    assert_eq!(
        dir,
        steamapps.join("music/Lords of the Fallen - Soundtrack")
    );
    assert!(is_music_install(&dir));
}

#[test]
fn games_stay_under_common() {
    let tmp = tempfile::tempdir().unwrap();
    let steamapps = tmp.path().join("steamapps");
    std::fs::create_dir_all(steamapps.join("common/Game")).unwrap();
    // Same name in both: common wins.
    std::fs::create_dir_all(steamapps.join("music/Game")).unwrap();
    let dir = resolve_install_dir(&steamapps, "Game");
    assert_eq!(dir, steamapps.join("common/Game"));
    assert!(!is_music_install(&dir));
}

#[test]
fn missing_folder_falls_back_to_common() {
    let tmp = tempfile::tempdir().unwrap();
    let steamapps = tmp.path().join("steamapps");
    assert_eq!(
        resolve_install_dir(&steamapps, "Gone"),
        steamapps.join("common/Gone")
    );
}

#[test]
fn music_named_game_folder_is_not_a_soundtrack() {
    assert!(!is_music_install(std::path::Path::new(
        "/lib/steamapps/common/music"
    )));
}
