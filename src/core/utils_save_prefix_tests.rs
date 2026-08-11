use super::*;

fn config_with_library(library: &std::path::Path) -> crate::core::config::LauncherConfig {
    let mut config = crate::core::config::LauncherConfig::default();
    config.steam_library_path = library.to_string_lossy().into_owned();
    // Default `Shared` mode: the launcher points WINEPREFIX at the master prefix
    // while Proton still runs the game out of compatdata. That split is the bug
    // this function exists to close.
    config.use_shared_compat_data = false;
    config
}

#[test]
fn proton_prefix_wins_once_proton_has_set_it_up() {
    // Proton ignores WINEPREFIX and uses $STEAM_COMPAT_DATA_PATH/pfx, so a game
    // launched through it reads and writes saves there — not in the configured
    // WINEPREFIX. `drive_c` existing is the evidence Proton actually ran.
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path();
    let proton_prefix = library.join("steamapps/compatdata/1903340/pfx");
    std::fs::create_dir_all(proton_prefix.join("drive_c/users/steamuser")).unwrap();

    let config = config_with_library(library);
    let store = crate::core::models::UserConfigStore::new();

    assert_eq!(game_save_prefix(&config, 1903340, &store), proton_prefix);
    // The configured WINEPREFIX still points elsewhere — that's the split.
    assert_ne!(
        steam_wineprefix_for_game(&config, 1903340, &store),
        proton_prefix
    );
}

#[test]
fn bare_compatdata_dir_is_not_enough() {
    // `prepare_prefix` pre-creates the compatdata directory for every launch,
    // Proton or not, so its mere existence proves nothing. Without `pfx/drive_c`
    // the configured WINEPREFIX is still the right answer.
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path();
    std::fs::create_dir_all(library.join("steamapps/compatdata/1903340")).unwrap();

    let config = config_with_library(library);
    let store = crate::core::models::UserConfigStore::new();

    assert_eq!(
        game_save_prefix(&config, 1903340, &store),
        steam_wineprefix_for_game(&config, 1903340, &store),
    );
}

#[test]
fn proton_compat_prefix_matches_steam_compat_data_path() {
    // Must stay in step with the STEAM_COMPAT_DATA_PATH the runner exports, or
    // saves land beside the prefix the game actually uses.
    let tmp = tempfile::tempdir().unwrap();
    let config = config_with_library(tmp.path());
    assert_eq!(
        proton_compat_prefix(&config, 42),
        tmp.path().join("steamapps").join("compatdata").join("42").join("pfx"),
    );
}
