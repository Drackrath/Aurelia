use super::super::*;
use crate::core::models::SteamPrefixMode;

#[test]
fn shared_inwine_resolves_to_master_pfx() {
    let master = PathBuf::from("/home/u/.config/Aurelia/master_steam_prefix/pfx");
    let got = game_compat_data_path(
        Path::new("/games"),
        123,
        SteamMode::InWineRuntime,
        SteamPrefixMode::Shared,
        &master,
    );
    // Proton appends /pfx to compat path.
    assert_eq!(got.join("pfx"), master);
    assert_eq!(
        got,
        PathBuf::from("/home/u/.config/Aurelia/master_steam_prefix")
    );
}

#[test]
fn per_game_inwine_uses_appid_compatdata() {
    let master = PathBuf::from("/home/u/.config/Aurelia/master_steam_prefix/pfx");
    let got = game_compat_data_path(
        Path::new("/games"),
        123,
        SteamMode::InWineRuntime,
        SteamPrefixMode::PerGame,
        &master,
    );
    assert_eq!(got, PathBuf::from("/games/steamapps/compatdata/123"));
}

#[test]
fn standalone_uses_appid_compatdata() {
    let master = PathBuf::from("/m/pfx");
    let got = game_compat_data_path(
        Path::new("/games"),
        123,
        SteamMode::Standalone,
        SteamPrefixMode::Shared,
        &master,
    );
    assert_eq!(got, PathBuf::from("/games/steamapps/compatdata/123"));
}

#[test]
fn root_layout_master_falls_back_to_appid() {
    // Non-pfx master layout can't share.
    let master = PathBuf::from("/home/u/.config/Aurelia/master_steam_prefix");
    let got = game_compat_data_path(
        Path::new("/games"),
        123,
        SteamMode::InWineRuntime,
        SteamPrefixMode::Shared,
        &master,
    );
    assert_eq!(got, PathBuf::from("/games/steamapps/compatdata/123"));
}
