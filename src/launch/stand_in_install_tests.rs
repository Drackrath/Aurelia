use super::*;
use StandInSteamInstall::{Auto, Host, InWine};

fn master(dir: &Path, with_steam: bool) -> PathBuf {
    let pfx = dir.join("master_steam_prefix/pfx");
    std::fs::create_dir_all(pfx.join("drive_c")).unwrap();
    if with_steam {
        std::fs::create_dir_all(pfx.join(PREFIX_STEAM_DIR)).unwrap();
    }
    pfx
}

#[test]
fn auto_prefers_in_wine_then_host() {
    let dir = tempfile::tempdir().unwrap();
    let root = PathBuf::from("/home/u/.local/share/Steam");
    let host = Some(root.clone());
    let ready = master(dir.path(), true);
    assert_eq!(
        choose(Auto, None, host.clone(), &ready),
        Ok(Install::InWine(ready.clone()))
    );
    let empty = tempfile::tempdir().unwrap();
    let bare = master(empty.path(), false);
    assert_eq!(
        choose(Auto, None, host.clone(), &bare),
        Ok(Install::Host(root))
    );
    let err = choose(Auto, None, None, &bare).unwrap_err();
    assert!(err.contains("steam-runtime install"), "{err}");
}

#[test]
fn explicit_choices_and_per_game_override() {
    let dir = tempfile::tempdir().unwrap();
    let ready = master(dir.path(), true);
    let host = PathBuf::from("/h/Steam");
    // Per-game beats global.
    assert_eq!(
        choose(InWine, Some(Host), Some(host.clone()), &ready),
        Ok(Install::Host(host.clone()))
    );
    assert_eq!(
        choose(Host, Some(InWine), Some(host.clone()), &ready),
        Ok(Install::InWine(ready.clone()))
    );
    assert!(
        choose(Host, None, None, &ready)
            .unwrap_err()
            .contains("--steam-install host")
    );
    let empty = tempfile::tempdir().unwrap();
    let bare = master(empty.path(), false);
    assert!(
        choose(InWine, None, Some(host), &bare)
            .unwrap_err()
            .contains("in-wine")
    );
}

#[test]
fn host_layout_points_proton_at_host_steam() {
    let compat = PathBuf::from("/lib/steamapps/compatdata/438100");
    let install = Install::Host(PathBuf::from("/h/Steam"));
    let l = layout(&install, &compat);
    assert_eq!(l.compat_data, compat);
    assert_eq!(l.prefix, compat.join("pfx"));
    assert_eq!(l.client_install, Some(PathBuf::from("/h/Steam")));
    let line = describe(&install, &l);
    assert!(
        line.contains("host") && line.contains("/h/Steam") && line.contains("438100/pfx"),
        "{line}"
    );
}

#[test]
fn in_wine_layout_keeps_one_prefix() {
    let master = PathBuf::from("/cfg/master_steam_prefix/pfx");
    let install = Install::InWine(master.clone());
    let l = layout(&install, Path::new("/lib/steamapps/compatdata/438100"));
    // Proton uses compat_data/pfx: the master.
    assert_eq!(l.compat_data, PathBuf::from("/cfg/master_steam_prefix"));
    assert_eq!(l.prefix, master);
    assert_eq!(l.client_install, None);
    assert!(describe(&install, &l).contains("fake trap"));
}

#[test]
fn root_layout_master_is_not_proton_ready() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("master_steam_prefix");
    std::fs::create_dir_all(root.join(PREFIX_STEAM_DIR)).unwrap();
    assert!(!in_wine_ready(&root));
    let host = PathBuf::from("/h/Steam");
    assert_eq!(
        choose(Auto, None, Some(host.clone()), &root),
        Ok(Install::Host(host))
    );
}

#[test]
fn settings_serialize_kebab_case() {
    assert_eq!(serde_json::to_string(&InWine).unwrap(), "\"in-wine\"");
    assert_eq!(
        serde_json::from_str::<StandInSteamInstall>("\"host\"").unwrap(),
        Host
    );
    assert_eq!(StandInSteamInstall::default(), Auto);
}
