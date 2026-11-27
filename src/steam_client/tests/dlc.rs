use super::super::*;

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

    assert_eq!(
        section.get_str(&["common", "name"]),
        Some("Vampire Survivors")
    );
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
fn installed_dlc_depots_reads_dlcappid_tags() {
    let raw = "\"AppState\"\n{\n\t\"InstalledDepots\"\n\t{\n\
        \t\t\"977954\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"1\"\n\t\t}\n\
        \t\t\"1977574\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"7\"\n\t\t\t\"dlcappid\"\t\t\"1977570\"\n\t\t}\n\
        \t}\n}\n";
    assert_eq!(
        manifests::installed_dlc_depots(raw),
        [1977574u64].into_iter().collect()
    );
}

fn api_error(result: steam_vent::EResult) -> anyhow::Error {
    anyhow::Error::new(steam_vent::NetworkError::ApiError(result)).context("ticket refused")
}

#[test]
fn rate_limited_ownership_check_is_inconclusive() {
    assert!(manifests::ownership_check_inconclusive(&api_error(
        steam_vent::EResult::RateLimitExceeded
    )));
}

#[test]
fn timed_out_ownership_check_is_inconclusive() {
    let err = anyhow::Error::new(steam_vent::NetworkError::Timeout);
    assert!(manifests::ownership_check_inconclusive(&err));
}

#[test]
fn denied_ownership_ticket_means_not_owned() {
    assert!(!manifests::ownership_check_inconclusive(&api_error(
        steam_vent::EResult::AccessDenied
    )));
    assert!(!manifests::ownership_check_inconclusive(&anyhow::anyhow!(
        "Steam returned an empty app ownership ticket for app 10"
    )));
}
