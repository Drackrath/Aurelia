use super::super::*;

fn config_with_prefix(library: &Path, dir: &Path) -> crate::core::config::LauncherConfig {
    let game = crate::core::config::GameConfig {
        prefix_dir: Some(dir.to_path_buf()),
        ..Default::default()
    };
    crate::core::config::LauncherConfig {
        steam_library_path: library.to_string_lossy().into_owned(),
        game_configs: [(1903340, game)].into(),
        ..Default::default()
    }
}

#[test]
fn custom_dir_replaces_compatdata() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Prefixes/Game");
    let config = config_with_prefix(tmp.path(), &dir);
    assert_eq!(game_compat_data_dir(&config, 1903340, None), dir);
    assert_eq!(
        proton_compat_prefix(&config, 1903340, None),
        dir.join("pfx")
    );
}

#[test]
fn wineprefix_and_saves_follow_custom_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Prefixes/Game");
    let config = config_with_prefix(tmp.path(), &dir);
    let store = crate::core::models::UserConfigStore::new();
    assert_eq!(
        steam_wineprefix_for_game(&config, 1903340, &store, None),
        dir.join("pfx")
    );
    assert_eq!(
        game_save_prefix(&config, 1903340, &store, None),
        dir.join("pfx")
    );
}

#[test]
fn other_games_keep_compatdata() {
    let tmp = tempfile::tempdir().unwrap();
    let config = config_with_prefix(tmp.path(), &tmp.path().join("Prefixes/Game"));
    assert_eq!(
        game_compat_data_dir(&config, 42, None),
        compat_data_dir(tmp.path(), 42)
    );
}
