use super::*;

// Unlikely to exist on real libraries.
const APP: u32 = 3_999_999_917;

fn config_with_library(library: &Path) -> crate::core::config::LauncherConfig {
    crate::core::config::LauncherConfig {
        steam_library_path: library.to_string_lossy().into_owned(),
        use_shared_compat_data: false,
        ..Default::default()
    }
}

fn write_manifest(library: &Path, app_id: u32) {
    let steamapps = library.join("steamapps");
    std::fs::create_dir_all(&steamapps).unwrap();
    std::fs::write(steamapps.join(format!("appmanifest_{app_id}.acf")), "").unwrap();
}

fn write_libraryfolders(root: &Path, libs: &[&Path]) {
    let mut body = String::from("\"libraryfolders\"\n{\n");
    for (i, lib) in libs.iter().enumerate() {
        body.push_str(&format!(
            "\t\"{i}\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n",
            lib.display()
        ));
    }
    body.push_str("}\n");
    let steamapps = root.join("steamapps");
    std::fs::create_dir_all(&steamapps).unwrap();
    std::fs::write(steamapps.join("libraryfolders.vdf"), body).unwrap();
}

#[test]
fn manifest_in_second_library_wins() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("A");
    let b = tmp.path().join("B");
    write_libraryfolders(&a, &[&a, &b]);
    write_manifest(&b, APP);
    assert_eq!(game_library_root_in(&a, &[], APP), b);

    let config = config_with_library(&a);
    assert_eq!(
        proton_compat_prefix(&config, APP, None),
        b.join(format!("steamapps/compatdata/{APP}/pfx"))
    );
}

#[test]
fn steam_root_library_is_found() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("A");
    let root = tmp.path().join("root");
    let b = tmp.path().join("B");
    std::fs::create_dir_all(&a).unwrap();
    write_libraryfolders(&root, &[&root, &b]);
    write_manifest(&b, APP);
    assert_eq!(game_library_root_in(&a, &[root], APP), b);
}

#[test]
fn manifest_only_in_configured_or_nowhere_keeps_configured() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("A");
    let b = tmp.path().join("B");
    write_libraryfolders(&a, &[&a, &b]);
    assert_eq!(game_library_root_in(&a, &[], APP), a);
    write_manifest(&a, APP);
    write_manifest(&b, APP);
    assert_eq!(game_library_root_in(&a, &[], APP), a);
}

#[test]
fn old_prefix_in_configured_still_uses_game_library() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("A");
    let b = tmp.path().join("B");
    write_libraryfolders(&a, &[&a, &b]);
    write_manifest(&b, APP);
    std::fs::create_dir_all(compat_data_dir(&a, APP).join("pfx/drive_c")).unwrap();
    let config = config_with_library(&a);
    // Hint is logged; B is used.
    assert_eq!(
        game_compat_data_dir(&config, APP, None),
        compat_data_dir(&b, APP)
    );
}

#[test]
fn save_prefix_follows_game_library() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("A");
    let b = tmp.path().join("B");
    write_libraryfolders(&a, &[&a, &b]);
    write_manifest(&b, APP);
    let pfx = compat_data_dir(&b, APP).join("pfx");
    std::fs::create_dir_all(pfx.join("drive_c")).unwrap();
    let config = config_with_library(&a);
    let store = crate::core::models::UserConfigStore::default();
    assert_eq!(game_save_prefix(&config, APP, &store, None), pfx);
}

#[test]
fn unregistered_library_found_via_install_path() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("A");
    let c = tmp.path().join("C");
    std::fs::create_dir_all(a.join("steamapps")).unwrap();
    write_manifest(&c, APP);
    let install = c.join("steamapps/common/Game");
    std::fs::create_dir_all(&install).unwrap();
    let config = config_with_library(&a);
    // Not in any libraryfolders.vdf.
    assert_eq!(game_library_root(&config, APP, None), a);
    assert_eq!(game_library_root(&config, APP, Some(&install)), c);
    assert_eq!(
        proton_compat_prefix(&config, APP, Some(&install)),
        compat_data_dir(&c, APP).join("pfx")
    );
    let pfx = compat_data_dir(&c, APP).join("pfx");
    std::fs::create_dir_all(pfx.join("drive_c")).unwrap();
    let store = crate::core::models::UserConfigStore::default();
    assert_eq!(game_save_prefix(&config, APP, &store, Some(&install)), pfx);
}

#[test]
fn install_path_needs_manifest_and_layout() {
    let tmp = tempfile::tempdir().unwrap();
    let c = tmp.path().join("C");
    let install = c.join("steamapps/common/Game");
    std::fs::create_dir_all(&install).unwrap();
    // No appmanifest yet.
    assert_eq!(library_from_install_path(&install, APP), None);
    write_manifest(&c, APP);
    assert_eq!(library_from_install_path(&install, APP), Some(c.clone()));
    // Wrong layout is ignored.
    let odd = c.join("games/Game");
    std::fs::create_dir_all(&odd).unwrap();
    assert_eq!(library_from_install_path(&odd, APP), None);
}
