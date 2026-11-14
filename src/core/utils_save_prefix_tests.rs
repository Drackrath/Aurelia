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

    assert_eq!(
        game_save_prefix(&config, 1903340, &store, None),
        proton_prefix
    );
    // The configured WINEPREFIX still points elsewhere — that's the split.
    assert_ne!(
        steam_wineprefix_for_game(&config, 1903340, &store, None),
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
        game_save_prefix(&config, 1903340, &store, None),
        steam_wineprefix_for_game(&config, 1903340, &store, None),
    );
}

#[test]
fn proton_compat_prefix_matches_steam_compat_data_path() {
    // Must stay in step with the STEAM_COMPAT_DATA_PATH the runner exports, or
    // saves land beside the prefix the game actually uses.
    let tmp = tempfile::tempdir().unwrap();
    let config = config_with_library(tmp.path());
    assert_eq!(
        proton_compat_prefix(&config, 42, None),
        tmp.path()
            .join("steamapps")
            .join("compatdata")
            .join("42")
            .join("pfx"),
    );
}

mod stand_in {
    use super::*;
    use crate::core::models::{StandInSteamInstall, SteamRuntimePolicy, UserAppConfig};

    const STEAM: &str = "drive_c/Program Files (x86)/Steam";

    fn on(install: StandInSteamInstall) -> crate::core::config::LauncherConfig {
        crate::core::config::LauncherConfig {
            steamworks_client: true,
            steamworks_steam_install: install,
            ..Default::default()
        }
    }

    #[test]
    fn in_wine_uses_the_master_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let master = dir.path().join("master_steam_prefix/pfx");
        std::fs::create_dir_all(master.join(STEAM)).unwrap();
        let old = dir.path().join("lib/steamapps/compatdata/438100/pfx");
        std::fs::create_dir_all(old.join("drive_c/users/steamuser")).unwrap();
        let got = stand_in_save_prefix(
            &on(StandInSteamInstall::Auto),
            None,
            &old,
            Some(dir.path().join("Steam")),
            &master,
        );
        assert_eq!(got, Some(master));
    }

    #[test]
    fn host_uses_the_compatdata_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let master = dir.path().join("master_steam_prefix/pfx");
        std::fs::create_dir_all(master.join(STEAM)).unwrap();
        let compat_pfx = dir.path().join("lib/steamapps/compatdata/438100/pfx");
        let per_game = UserAppConfig {
            stand_in_steam_install: Some(StandInSteamInstall::Host),
            ..Default::default()
        };
        let got = stand_in_save_prefix(
            &on(StandInSteamInstall::InWine),
            Some(&per_game),
            &compat_pfx,
            Some(dir.path().join("Steam")),
            &master,
        );
        assert_eq!(got, Some(compat_pfx));
    }

    #[test]
    fn off_explicit_or_unresolvable_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let master = dir.path().join("none/pfx");
        let pfx = dir.path().join("compat/pfx");
        let off = crate::core::config::LauncherConfig::default();
        assert_eq!(stand_in_save_prefix(&off, None, &pfx, None, &master), None);
        let runtime = UserAppConfig {
            steam_runtime_policy: SteamRuntimePolicy::Enabled,
            ..Default::default()
        };
        let cfg = on(StandInSteamInstall::Host);
        let host = Some(dir.path().join("Steam"));
        assert_eq!(
            stand_in_save_prefix(&cfg, Some(&runtime), &pfx, host, &master),
            None
        );
        // Nothing installed: old rules.
        assert_eq!(stand_in_save_prefix(&cfg, None, &pfx, None, &master), None);
    }
}
