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
        AppManifestSpec {
            appid: 960090,
            game_name: "Bloons TD 6",
            installdir: "BloonsTD6",
            installed_depots: vec![(960091, 3975959124549939908, 2805466046)],
            buildid: Some("24771151"),
            fully_installed: true,
            pin_updates: false,
            language: None,
        },
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
    SteamClient::write_appmanifest(
        &path,
        AppManifestSpec {
            appid: 1,
            game_name: "Game",
            installdir: "Game",
            installed_depots: vec![],
            buildid: None,
            fully_installed: false,
            pin_updates: false,
            language: None,
        },
    )
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
        AppManifestSpec {
            appid: 1086940,
            game_name: "Baldur's Gate 3",
            installdir: "Baldurs Gate 3",
            installed_depots: vec![(2378505, 5, 50)],
            buildid: Some("1"),
            fully_installed: true,
            pin_updates: false,
            language: Some("english"),
        },
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
        AppManifestSpec {
            appid: 1,
            game_name: "Game",
            installdir: "Game",
            installed_depots: vec![],
            buildid: None,
            fully_installed: true,
            pin_updates: false,
            language: Some("german"),
        },
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
        AppManifestSpec {
            appid: 977950,
            game_name: "A Dance of Fire and Ice",
            installdir: "A Dance of Fire and Ice",
            installed_depots: vec![(977954, 2, 20), (1977574, 9, 90)],
            buildid: Some("26397875"),
            fully_installed: true,
            pin_updates: false,
            language: None,
        },
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
    SteamClient::write_appmanifest(
        &path,
        AppManifestSpec {
            appid: 1,
            game_name: "Game",
            installdir: "Game",
            installed_depots: vec![],
            buildid: None,
            fully_installed: false,
            pin_updates: false,
            language: None,
        },
    )
    .unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("\"TargetBuildID\"\t\t\"55\""));
}
