use super::*;

const PICS: &str = r#""appinfo"
{
    "depots"
    {
        "481" { "name" "base" }
        "482" { "dlcappid" "1001" }
        "483" { "dlcappid" " 1002 " }
        "branches" { "public" { "buildid" "9" } }
    }
}"#;

fn set(ids: &[u32]) -> HashSet<u32> {
    ids.iter().copied().collect()
}

#[test]
fn depot_backed_dlcs_are_found() {
    let vdf = steam_vdf_parser::parse_text(PICS).unwrap();
    let section = pics_app_section(vdf.value());
    assert_eq!(dlcs_with_depots(section), set(&[1001, 1002]));
}

#[test]
fn dlc_list_joins_sources() {
    let names: HashMap<u32, String> = [(1001, "Soundtrack".to_string()), (1003, String::new())]
        .into_iter()
        .collect();
    let list = build_dlc_list(
        &[1001, 1002, 1003],
        &set(&[1001, 1002]),
        &set(&[1001]),
        &set(&[1003]),
        &names,
    );
    assert_eq!(
        list,
        vec![
            DlcInfo {
                app_id: 1001,
                name: Some("Soundtrack".into()),
                content_ready: true,
                disabled: false,
            },
            DlcInfo {
                app_id: 1002,
                name: None,
                content_ready: false,
                disabled: false,
            },
            DlcInfo {
                app_id: 1003,
                name: None,
                content_ready: true,
                disabled: true,
            },
        ]
    );
}

#[test]
fn manifest_language_is_read() {
    let raw = "\"AppState\"\n{\n\t\"appid\"\t\t\"480\"\n\t\"buildid\"\t\t\"77\"\n\t\"UserConfig\"\n\t{\n\t\t\"language\"\t\t\"german\"\n\t}\n}\n";
    let m = crate::core::acf::parse_app_manifest(raw);
    assert_eq!(m.language.as_deref(), Some("german"));
    assert_eq!(m.build_id, 77);
    let none = crate::core::acf::parse_app_manifest("\"AppState\"\n{\n}\n");
    assert_eq!(none.language, None);
}

#[test]
fn dlc_catalog_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(load_dlc_catalog(dir.path(), 480), None);
    let catalog = DlcCatalog {
        ids: vec![1001, 1002],
        with_depots: vec![1002],
        names: HashMap::from([(1001, "OST".to_string())]),
    };
    store_dlc_catalog(dir.path(), 480, &catalog).unwrap();
    assert_eq!(load_dlc_catalog(dir.path(), 480), Some(catalog));
    assert_eq!(load_dlc_catalog(dir.path(), 70), None);
    std::fs::write(dlc_catalog_path(dir.path(), 70), b"not json").unwrap();
    assert_eq!(load_dlc_catalog(dir.path(), 70), None);
}
