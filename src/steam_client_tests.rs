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
