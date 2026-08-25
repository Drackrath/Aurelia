use super::*;

#[test]
fn fully_installed_only_when_state_flag_set() {
    // StateFlags 4 = StateFullyInstalled.
    assert!(manifest_is_fully_installed(
        "\"AppState\"\n{\n\t\"StateFlags\"\t\t\"4\"\n}"
    ));
    // 6 = StateFullyInstalled | StateUpdateRequired (installed, update pending).
    assert!(manifest_is_fully_installed(
        "\"AppState\"\n{\n\t\"StateFlags\"\t\t\"6\"\n}"
    ));
    // 2 = StateUpdateRequired only: an install that started but never finished
    // (e.g. cancelled). Must NOT count as installed.
    assert!(!manifest_is_fully_installed(
        "\"AppState\"\n{\n\t\"StateFlags\"\t\t\"2\"\n}"
    ));
    // Missing StateFlags is treated as not installed.
    assert!(!manifest_is_fully_installed("\"AppState\"\n{\n}"));
}

#[test]
fn update_pending_when_update_required_flag_set() {
    // 4 = StateFullyInstalled only: up to date, no update pending.
    assert!(!manifest_update_pending(
        "\"AppState\"\n{\n\t\"StateFlags\"\t\t\"4\"\n}"
    ));
    // 6 = StateFullyInstalled | StateUpdateRequired: installed, update pending.
    assert!(manifest_update_pending(
        "\"AppState\"\n{\n\t\"StateFlags\"\t\t\"6\"\n}"
    ));
    // 1046 = a partially-started update
    assert!(manifest_update_pending(
        "\"AppState\"\n{\n\t\"StateFlags\"\t\t\"1046\"\n}"
    ));
    // Missing StateFlags
    assert!(!manifest_update_pending("\"AppState\"\n{\n}"));
}

fn cats(pairs: &[(u32, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(id, v)| (format!("category_{id}"), v.to_string()))
        .collect()
}

#[test]
fn online_required_mmo_without_single_player() {
    // MMO (20), no single-player => requires online.
    assert!(category_online_required(&cats(&[(20, "1"), (1, "1")])));
}

#[test]
fn online_required_online_coop_without_single_player() {
    // Online Co-op (38) only => requires online.
    assert!(category_online_required(&cats(&[(38, "1")])));
}

#[test]
fn not_online_required_when_single_player_present() {
    // Online PvP (36) but also Single-player (2) => playable offline.
    assert!(!category_online_required(&cats(&[(36, "1"), (2, "1")])));
}

#[test]
fn not_online_required_for_local_multiplayer_only() {
    // Generic Multi-player (1) / Shared-Split-Screen (24) are not online-only.
    assert!(!category_online_required(&cats(&[(1, "1"), (24, "1")])));
}

#[test]
fn not_online_required_when_categories_absent_or_zeroed() {
    assert!(!category_online_required(&cats(&[])));
    assert!(!category_online_required(&cats(&[(20, "0"), (2, "0")])));
}

#[test]
fn unix_to_ymd_known_dates() {
    assert_eq!(unix_to_ymd(0), "1970-01-01");
    assert_eq!(unix_to_ymd(1_700_000_000), "2023-11-14"); // 2023-11-14T22:13:20Z
    assert_eq!(unix_to_ymd(1_009_843_200), "2002-01-01"); // exact midnight UTC
    // Leap day round-trips correctly.
    assert_eq!(unix_to_ymd(1_582_934_400), "2020-02-29");
}

#[test]
fn achievement_icon_urls() {
    assert_eq!(achievement_icon_url(440, ""), "");
    assert_eq!(
        achievement_icon_url(440, "abc123.jpg"),
        "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/440/abc123.jpg"
    );
    // An already-absolute URL is passed through unchanged.
    assert_eq!(
        achievement_icon_url(440, "https://example.com/i.png"),
        "https://example.com/i.png"
    );
}

#[test]
fn store_app_type_labels() {
    assert_eq!(store_app_type_label(EStoreAppType::k_EStoreAppType_Game), "Game");
    assert_eq!(store_app_type_label(EStoreAppType::k_EStoreAppType_DLC), "DLC");
    assert_eq!(store_app_type_label(EStoreAppType::k_EStoreAppType_Music), "Soundtrack");
}

#[tokio::test]
async fn test_legacy_path_blocks_windows_proton() {
    let client = SteamClient::new().unwrap();
    let app = LibraryGame {
        app_id: 123,
        name: "Test Game".to_string(),
        install_path: Some("/tmp/test_game".to_string()),
        is_installed: true,
        playtime_forever_minutes: Some(0),
        active_branch: "public".to_string(),
        update_available: false,
        update_queued: false,
        local_manifest_ids: HashMap::new(),
        is_owned: true,
        is_family_shared: false,
        online_required: None,
        platform: None,
        from_windows_steam: false,
    };
    let launch_info = LaunchInfo {
        app_id: 123,
        id: "0".to_string(),
        description: "Test".to_string(),
        executable: "test.exe".to_string(),
        arguments: "".to_string(),
        workingdir: None,
        target: LaunchTarget::WindowsProton,
    };
    let config = crate::core::config::LauncherConfig::default();

    let result = client.internal_legacy_launch_adhoc(&app, &launch_info, None, &config, None).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Ad-hoc bypass is prohibited"));
}

#[tokio::test]
async fn test_pipeline_integration_scaffolding() {
    // Passing no app causes ResolveGame to fail early.
    let mut ctx = crate::launch::pipeline::PipelineContext::new(999999);
    let pipeline = crate::launch::pipeline::LaunchPipeline::with_default_stages();

    let result = pipeline.run(&mut ctx).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.stage_name, "ResolveGame");
    assert!(err.inner.to_string().contains("App context missing"));
}

#[test]
fn parses_installed_and_disabled_dlc_from_appmanifest() {
    // Base game with two DLC depots installed (tagged with dlcappid) and one of
    // those DLC explicitly disabled.
    let manifest = r#""AppState"
{
	"appid"		"1000"
	"InstalledDepots"
	{
		"1001"
		{
			"manifest"	"123"
			"size"		"456"
			"dlcappid"	"2001"
		}
		"1002"
		{
			"manifest"	"789"
			"size"		"12"
			"dlcappid"	"2002"
		}
	}
	"UserConfig"
	{
		"DisabledDLC"		"2002"
	}
}
"#;

    let installed = parse_installed_dlc_appids(manifest);
    assert!(installed.contains(&2001));
    assert!(installed.contains(&2002));
    assert_eq!(installed.len(), 2);

    let disabled = parse_disabled_dlc_appids(manifest);
    assert_eq!(disabled, HashSet::from([2002]));
}

#[test]
fn parses_comma_separated_disabled_dlc_list() {
    let manifest = r#""AppState"
{
	"MountedConfig"
	{
		"DisabledDLC"		"3001,3002, 3003"
	}
}
"#;
    let disabled = parse_disabled_dlc_appids(manifest);
    assert_eq!(disabled, HashSet::from([3001, 3002, 3003]));
    assert!(parse_installed_dlc_appids(manifest).is_empty());
}

#[test]
fn parses_linux_launch_section_from_vdf() {
    let raw = r#""appinfo"
{
  "appid" "10"
  "config"
  {
"launch"
{
  "0"
  {
    "executable" "linux/game.sh"
    "arguments" "-foo -bar"
    "oslist" "linux"
  }
}
  }
}"#;

    let launch_options = parse_launch_info_from_vdf(10, raw).expect("parse launch info");
    let launch = &launch_options[0];
    assert_eq!(launch.target, LaunchTarget::NativeLinux);
    assert_eq!(launch.executable, "linux/game.sh");
    assert_eq!(launch.arguments, "-foo -bar");
}

#[test]
fn extracts_dlc_ids_from_listofdlc() {
    // Mirrors a real PICS appinfo: sections nested under an appid-keyed root,
    // DLC declared in `extended/listofdlc`. Regression guard for the daemon
    // returning an empty DLC list when appinfo isn't the text-only shape.
    let raw = r#""1794680"
{
  "common" { "name" "Vampire Survivors" }
  "extended" { "listofdlc" "2305610,2305620, 2305630,2305640,2305650" }
}"#;
    let vdf = find_vdf_in_pics(raw.as_bytes()).expect("parse pics vdf");
    let section = pics_app_section(vdf.value());

    assert_eq!(section.get_str(&["common", "name"]), Some("Vampire Survivors"));
    assert_eq!(
        dlc_ids_from_section(section),
        vec![2305610, 2305620, 2305630, 2305640, 2305650],
    );
}

#[test]
fn dlc_ids_empty_when_no_listofdlc() {
    let raw = r#""appinfo" { "common" { "name" "No DLC Game" } }"#;
    let vdf = find_vdf_in_pics(raw.as_bytes()).expect("parse pics vdf");
    let section = pics_app_section(vdf.value());
    assert!(dlc_ids_from_section(section).is_empty());
}

#[test]
fn parses_ufs_savefile_rules() {
    let raw = r#""2784470"
{
  "ufs"
  {
"savefiles"
{
  "0"
  {
    "root" "WinAppDataLocalLow"
    "path" "SadSocket/9Kings"
    "pattern" "*"
    "recursive" "1"
  }
  "1"
  {
    "root" "GameInstall"
    "path" "Saves"
    "pattern" "*.sav"
  }
}
  }
}"#;
    let vdf = find_vdf_in_pics(raw.as_bytes()).expect("parse pics vdf");
    let specs = ufs_save_specs_from_section(pics_app_section(vdf.value()));
    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].root, "WinAppDataLocalLow");
    assert_eq!(specs[0].path, "SadSocket/9Kings");
    assert!(specs[0].recursive);
    assert_eq!(specs[1].root, "GameInstall");
    assert_eq!(specs[1].pattern, "*.sav");
    assert!(!specs[1].recursive); // absent recursive defaults to false
}

#[test]
fn acf_name_and_last_owner_parse() {
    let raw = "\"AppState\"\n{\n\t\"appid\"\t\t\"960090\"\n\t\"name\"\t\t\"Bloons TD 6\"\n\t\"LastOwner\"\t\t\"76561198000000001\"\n}\n";
    assert_eq!(parse_name_from_acf(raw).as_deref(), Some("Bloons TD 6"));
    assert_eq!(parse_last_owner_from_acf(raw), Some(76561198000000001));
    assert_eq!(parse_last_owner_from_acf("\"LastOwner\"\t\"0\""), None);
    assert_eq!(parse_name_from_acf("\"name\"\t\"\""), None);
}

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
    )
    .unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(parse_last_owner_from_acf(&raw), Some(76561198000000001));
    assert_eq!(parse_active_branch_from_acf(&raw), "no-discord");
    assert_eq!(parse_installdir_from_acf(&raw).as_deref(), Some("BloonsTD6"));
    assert_eq!(parse_name_from_acf(&raw).as_deref(), Some("Bloons TD 6"));
    assert!(manifest_is_fully_installed(&raw));
    assert_eq!(parse_installed_depots_from_acf(&raw).get(&960091), Some(&3975959124549939908));
}

#[test]
fn write_appmanifest_fresh_has_no_owner_or_branch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("appmanifest_1.acf");
    SteamClient::write_appmanifest(&path, 1, "Game", "Game", vec![], None, false, false).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(parse_last_owner_from_acf(&raw), None);
    assert_eq!(parse_active_branch_from_acf(&raw), "public");
    assert!(!raw.contains("UserConfig"));
}

#[test]
fn select_launch_entry_prefers_installed_platform() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("hl.exe"), "x").unwrap();
    std::fs::write(tmp.path().join("hl.sh"), "x").unwrap();

    let entry = |id: &str, exe: &str, target: LaunchTarget| LaunchInfo {
        app_id: 70,
        id: id.to_string(),
        description: String::new(),
        executable: exe.to_string(),
        arguments: String::new(),
        workingdir: None,
        target,
    };
    let options = vec![
        entry("0", "hl.exe", LaunchTarget::WindowsProton),
        entry("1", "hl.sh", LaunchTarget::NativeLinux),
    ];
    let mut app = LibraryGame {
        app_id: 70,
        name: "Half-Life".to_string(),
        install_path: Some(tmp.path().to_string_lossy().to_string()),
        is_installed: true,
        playtime_forever_minutes: None,
        active_branch: "public".to_string(),
        update_available: false,
        update_queued: false,
        local_manifest_ids: HashMap::new(),
        is_owned: true,
        is_family_shared: false,
        online_required: None,
        platform: Some("linux".to_string()),
        from_windows_steam: false,
    };

    // Installed platform wins over declared order and stale files.
    let picked = launch::select_launch_entry(&options, &app, false, false).unwrap();
    assert_eq!(picked.id, "1");

    // Explicit Proton/Windows request still picks the Windows entry.
    let picked = launch::select_launch_entry(&options, &app, true, false).unwrap();
    assert_eq!(picked.id, "0");

    // Explicit native preference beats the windows manifest.
    app.platform = Some("windows".to_string());
    let picked = launch::select_launch_entry(&options, &app, false, true).unwrap();
    assert_eq!(picked.id, "1");

    // Unknown installed platform: first existing executable.
    app.platform = None;
    let picked = launch::select_launch_entry(&options, &app, false, false).unwrap();
    assert_eq!(picked.id, "0");
}

#[test]
fn encrypted_ticket_refusal_maps_eresult() {
    use steam_vent_proto::steammessages_clientserver::CMsgClientRequestEncryptedAppTicketResponse;

    let mut response = CMsgClientRequestEncryptedAppTicketResponse::new();
    response.set_eresult(15);
    assert_eq!(
        client::extract_encrypted_ticket(&response, 400).unwrap(),
        EncryptedTicketOutcome::Refused(15)
    );

    // Missing eresult defaults to 2.
    let response = CMsgClientRequestEncryptedAppTicketResponse::new();
    assert_eq!(
        client::extract_encrypted_ticket(&response, 400).unwrap(),
        EncryptedTicketOutcome::Refused(2)
    );
}

#[test]
fn encrypted_ticket_success_keeps_envelope() {
    use protobuf::Message;
    use steam_vent_proto::encrypted_app_ticket::EncryptedAppTicket;
    use steam_vent_proto::steammessages_clientserver::CMsgClientRequestEncryptedAppTicketResponse;

    let mut envelope = EncryptedAppTicket::new();
    envelope.set_ticket_version_no(4);
    envelope.set_crc_encryptedticket(0xDEAD_BEEF);
    envelope.set_cb_encrypteduserdata(16);
    envelope.set_cb_encrypted_appownershipticket(96);
    envelope.set_encrypted_ticket(vec![7u8; 128]);

    let mut response = CMsgClientRequestEncryptedAppTicketResponse::new();
    response.set_eresult(1);
    response.encrypted_app_ticket = protobuf::MessageField::some(envelope);

    let EncryptedTicketOutcome::Issued(bytes) =
        client::extract_encrypted_ticket(&response, 945360).unwrap()
    else {
        panic!("expected an issued ticket");
    };

    // Round-trip: every envelope field survives.
    let parsed = EncryptedAppTicket::parse_from_bytes(&bytes).unwrap();
    assert_eq!(parsed.ticket_version_no(), 4);
    assert_eq!(parsed.crc_encryptedticket(), 0xDEAD_BEEF);
    assert_eq!(parsed.cb_encrypteduserdata(), 16);
    assert_eq!(parsed.cb_encrypted_appownershipticket(), 96);
    assert_eq!(parsed.encrypted_ticket(), &[7u8; 128][..]);
}

#[test]
fn encrypted_ticket_ok_without_ticket_errors() {
    use steam_vent_proto::encrypted_app_ticket::EncryptedAppTicket;
    use steam_vent_proto::steammessages_clientserver::CMsgClientRequestEncryptedAppTicketResponse;

    // OK but no envelope at all.
    let mut response = CMsgClientRequestEncryptedAppTicketResponse::new();
    response.set_eresult(1);
    assert!(client::extract_encrypted_ticket(&response, 400).is_err());

    // OK but the ciphertext is empty.
    let mut response = CMsgClientRequestEncryptedAppTicketResponse::new();
    response.set_eresult(1);
    response.encrypted_app_ticket = protobuf::MessageField::some(EncryptedAppTicket::new());
    assert!(client::extract_encrypted_ticket(&response, 400).is_err());
}
