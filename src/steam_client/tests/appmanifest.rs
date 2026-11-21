use super::super::*;
use crate::core::acf;

#[test]
fn write_appmanifest_preserves_owner_and_branch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("appmanifest_960090.acf");
    let existing = "\"AppState\"\n{\n\t\"appid\"\t\t\"960090\"\n\t\"name\"\t\t\"Bloons TD 6\"\n\
        \t\"installdir\"\t\t\"BloonsTD6\"\n\t\"LastOwner\"\t\t\"76561198000000001\"\n\
        \t\"UserConfig\"\n\t{\n\t\t\"betakey\"\t\t\"no-discord\"\n\t}\n}\n";
    std::fs::write(&path, existing).unwrap();

    SteamClient::write_appmanifest(
        &path,
        960090,
        "Bloons TD 6",
        "BloonsTD6",
        vec![(960091, 3975959124549939908, 2805466046)],
        Some("24771151"),
        true,
        false,
        None,
    )
    .unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        acf::parse_app_manifest(&raw).last_owner,
        Some(76561198000000001)
    );
    assert_eq!(acf::parse_app_manifest(&raw).active_branch, "no-discord");
    assert_eq!(
        acf::parse_app_manifest(&raw).install_dir.as_deref(),
        Some("BloonsTD6")
    );
    assert_eq!(
        acf::parse_app_manifest(&raw).display_name().as_deref(),
        Some("Bloons TD 6")
    );
    assert!(acf::parse_app_manifest(&raw).fully_installed());
    assert_eq!(
        acf::installed_depots(&raw).get(&960091),
        Some(&3975959124549939908)
    );
}

#[test]
fn write_appmanifest_fresh_has_no_owner_or_branch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("appmanifest_1.acf");
    SteamClient::write_appmanifest(&path, 1, "Game", "Game", vec![], None, false, false, None)
        .unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(acf::parse_app_manifest(&raw).last_owner, None);
    assert_eq!(acf::parse_app_manifest(&raw).active_branch, "public");
    assert!(!raw.contains("UserConfig"));
}

#[test]
fn write_appmanifest_keeps_user_and_mounted_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("appmanifest_1086940.acf");
    let existing = "\"AppState\"\n{\n\t\"appid\"\t\t\"1086940\"\n\t\"StateFlags\"\t\t\"6\"\n\
        \t\"UserConfig\"\n\t{\n\t\t\"language\"\t\t\"german\"\n\t\t\"DisabledDLC\"\t\t\"2378400,2378401\"\n\
        \t\t\"platform_override_dest\"\t\t\"linux\"\n\t\t\"platform_override_source\"\t\t\"windows\"\n\
        \t\t\"betakey\"\t\t\"beta\"\n\t}\n\
        \t\"MountedConfig\"\n\t{\n\t\t\"language\"\t\t\"german\"\n\t\t\"DisabledDLC\"\t\t\"2378400\"\n\t}\n}\n";
    std::fs::write(&path, existing).unwrap();

    SteamClient::write_appmanifest(
        &path,
        1086940,
        "Baldur's Gate 3",
        "Baldurs Gate 3",
        vec![(2378505, 5, 50)],
        Some("1"),
        true,
        false,
        Some("english"),
    )
    .unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("\"DisabledDLC\"\t\t\"2378400,2378401\""));
    assert!(raw.contains("\"platform_override_dest\"\t\t\"linux\""));
    assert!(raw.contains("\"platform_override_source\"\t\t\"windows\""));
    assert!(raw.contains("\"MountedConfig\""));
    // Steam's own choice wins.
    assert_eq!(
        manifests::user_config_language(&raw).as_deref(),
        Some("german")
    );
    assert_eq!(acf::parse_app_manifest(&raw).active_branch, "beta");
    assert_eq!(raw.matches("\"UserConfig\"").count(), 1);
    assert!(acf::parse_app_manifest(&raw).fully_installed());
}

#[test]
fn write_appmanifest_records_language_on_fresh_install() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("appmanifest_1.acf");
    SteamClient::write_appmanifest(
        &path,
        1,
        "Game",
        "Game",
        vec![],
        None,
        true,
        false,
        Some("german"),
    )
    .unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        manifests::user_config_language(&raw).as_deref(),
        Some("german")
    );
}

#[test]
fn write_appmanifest_keeps_steam_fields_and_dlc_tags() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("appmanifest_977950.acf");
    let existing = "\"AppState\"\n{\n\t\"appid\"\t\t\"977950\"\n\t\"StateFlags\"\t\t\"6\"\n\
        \t\"LastPlayed\"\t\t\"1781732299\"\n\t\"DownloadType\"\t\t\"2\"\n\
        \t\"TargetBuildID\"\t\t\"26397875\"\n\t\"LastOwner\"\t\t\"76561198000000001\"\n\
        \t\"InstalledDepots\"\n\t{\n\
        \t\t\"977954\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"1\"\n\t\t\t\"size\"\t\t\"10\"\n\t\t}\n\
        \t\t\"1977574\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"7\"\n\t\t\t\"size\"\t\t\"70\"\n\t\t\t\"dlcappid\"\t\t\"1977570\"\n\t\t}\n\
        \t\t\"1977584\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"8\"\n\t\t\t\"size\"\t\t\"80\"\n\t\t\t\"dlcappid\"\t\t\"1977580\"\n\t\t}\n\
        \t}\n\
        \t\"SharedDepots\"\n\t{\n\t\t\"228988\"\t\t\"228980\"\n\t}\n}\n";
    std::fs::write(&path, existing).unwrap();

    // 1977584 was not re-downloaded.
    SteamClient::write_appmanifest(
        &path,
        977950,
        "A Dance of Fire and Ice",
        "A Dance of Fire and Ice",
        vec![(977954, 2, 20), (1977574, 9, 90)],
        Some("26397875"),
        true,
        false,
        None,
    )
    .unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("\"LastPlayed\"\t\t\"1781732299\""));
    assert!(raw.contains("\"DownloadType\"\t\t\"2\""));
    assert!(raw.contains("\"TargetBuildID\"\t\t\"0\""));
    assert_eq!(raw.matches("TargetBuildID").count(), 1);
    assert_eq!(raw.matches("LastOwner").count(), 1);
    assert_eq!(raw.matches("StateFlags").count(), 1);
    assert!(raw.contains("\"SharedDepots\""));
    let depots = acf::installed_depots(&raw);
    assert_eq!(depots.get(&977954), Some(&2));
    assert_eq!(depots.get(&1977574), Some(&9));
    assert_eq!(depots.get(&1977584), Some(&8));
    // 20 + 90 written, 80 kept.
    assert!(raw.contains("\"SizeOnDisk\"\t\t\"190\""));
    assert_eq!(
        parse_installed_dlc_appids(&raw),
        [1977570, 1977580].into_iter().collect()
    );
    assert!(acf::parse_app_manifest(&raw).fully_installed());
}

#[test]
fn write_appmanifest_keeps_target_build_while_updating() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("appmanifest_1.acf");
    std::fs::write(
        &path,
        "\"AppState\"\n{\n\t\"appid\"\t\t\"1\"\n\t\"TargetBuildID\"\t\t\"55\"\n}\n",
    )
    .unwrap();
    SteamClient::write_appmanifest(&path, 1, "Game", "Game", vec![], None, false, false, None)
        .unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("\"TargetBuildID\"\t\t\"55\""));
}
