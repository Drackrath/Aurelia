use super::super::*;
use crate::core::error::{ErrorKind, classify};

fn parse(text: &str) -> steam_vdf_parser::Vdf<'static> {
    steam_vdf_parser::parse_text(text).unwrap().into_owned()
}

/// Untagged base depots only.
const NO_LANGUAGE: &str = r#""depots" {
    "11" { "config" { "oslist" "windows" } }
    "12" { "config" { "oslist" "linux" } }
    "branches" { "public" { "buildid" "1" } }
}"#;

/// Base plus DLC language depots.
const WITH_DLC: &str = r#""depots" {
    "21" { "config" { "oslist" "windows" } }
    "22" { "config" { "oslist" "windows" "language" "German" } }
    "23" { "config" { "oslist" "windows" "language" "french" } }
    "24" { "config" { "oslist" "linux" "language" "schinese" } }
    "25" { "config" { "language" "german" } "dlcappid" "20" }
    "31" { "config" { "oslist" "windows" "language" "polish" } "dlcappid" "30" }
    "32" { "config" { "oslist" "windows" "language" "german" } "dlcappid" "30" }
}"#;

#[test]
fn no_language_depots_list_base_language() {
    let vdf = parse(NO_LANGUAGE);
    let rows = vdf.as_obj().unwrap();
    for platform in [DepotPlatform::Windows, DepotPlatform::Linux] {
        assert_eq!(language_choices(rows, 10, platform), ["english"]);
        assert_eq!(default_language(rows, 10, platform, "german"), "english");
    }
}

#[test]
fn base_and_dlc_language_depots() {
    let vdf = parse(WITH_DLC);
    let rows = vdf.as_obj().unwrap();
    // DLC-only polish is not offered.
    assert_eq!(
        language_choices(rows, 20, DepotPlatform::Windows),
        ["english", "french", "german"]
    );
    assert_eq!(
        default_language(rows, 20, DepotPlatform::Windows, "German"),
        "german"
    );
    assert_eq!(
        default_language(rows, 20, DepotPlatform::Windows, "polish"),
        "english"
    );
}

#[test]
fn platform_filters_language_choices() {
    let vdf = parse(WITH_DLC);
    let rows = vdf.as_obj().unwrap();
    assert_eq!(
        language_choices(rows, 20, DepotPlatform::Linux),
        ["english", "german", "schinese"]
    );
    assert!(validate_language(rows, 20, DepotPlatform::Linux, "french").is_err());
    assert!(validate_language(rows, 20, DepotPlatform::Windows, "schinese").is_err());
}

#[test]
fn language_flag_is_normalised() {
    let vdf = parse(WITH_DLC);
    let rows = vdf.as_obj().unwrap();
    let lang = validate_language(rows, 20, DepotPlatform::Windows, " FRENCH ").unwrap();
    assert_eq!(lang, "french");
    let lang = validate_language(rows, 20, DepotPlatform::Windows, "English").unwrap();
    assert_eq!(lang, "english");
}

#[test]
fn unknown_language_is_typed_and_lists_choices() {
    let vdf = parse(WITH_DLC);
    let rows = vdf.as_obj().unwrap();
    let err = validate_language(rows, 20, DepotPlatform::Windows, "Polish").unwrap_err();
    assert_eq!(classify(&err).kind, ErrorKind::InvalidInput);
    let msg = err.to_string();
    assert!(msg.contains("no polish depots"), "{msg}");
    assert!(msg.contains("english, french, german"), "{msg}");

    let err = validate_language(rows, 20, DepotPlatform::Windows, "  ").unwrap_err();
    assert_eq!(classify(&err).kind, ErrorKind::InvalidInput);
}

#[test]
fn no_language_game_rejects_other_languages() {
    let vdf = parse(NO_LANGUAGE);
    let rows = vdf.as_obj().unwrap();
    let err = validate_language(rows, 10, DepotPlatform::Windows, "german").unwrap_err();
    assert!(err.to_string().contains("available: english"), "{err}");
}

#[test]
fn explicit_choice_is_recorded() {
    let vdf = parse(WITH_DLC);
    let rows = vdf.as_obj().unwrap();
    let win = DepotPlatform::Windows;
    // Untagged English counts when chosen.
    assert_eq!(
        recorded_language(rows, 20, win, "english", true).as_deref(),
        Some("english")
    );
    assert_eq!(recorded_language(rows, 20, win, "english", false), None);
    assert_eq!(
        recorded_language(rows, 20, win, "german", true).as_deref(),
        Some("german")
    );
}

#[test]
fn explicit_english_without_language_depots_is_not_recorded() {
    let vdf = parse(NO_LANGUAGE);
    let rows = vdf.as_obj().unwrap();
    assert_eq!(
        recorded_language(rows, 10, DepotPlatform::Windows, "english", true),
        None
    );
}
