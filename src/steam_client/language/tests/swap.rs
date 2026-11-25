use super::super::*;
use crate::core::acf;

const ROWS: &str = r#""depots"
{
	"2330359" { "config" { "oslist" "linux" } }
	"2378501" { "config" { "oslist" "linux" } }
	"2378503" { "config" { "oslist" "linux" "language" "english" } }
	"2378505" { "config" { "oslist" "linux" "language" "german" } }
	"2378507" { "config" { "oslist" "linux" "language" "french" } }
	"1086944" { "config" { "oslist" "windows" "language" "german" } }
	"9999" { "config" { "oslist" "linux" "language" "german" } "dlcappid" "5555" }
	"9998" { "config" { "oslist" "linux" "language" "english" } "dlcappid" "5555" }
	"7001" { "config" { "oslist" "linux" "language" "german" } "dlcappid" "7000" }
}
"#;

fn rows() -> steam_vdf_parser::Vdf<'static> {
    steam_vdf_parser::parse_text(ROWS).unwrap().into_owned()
}

fn map(pairs: &[(u64, u64)]) -> HashMap<u64, u64> {
    pairs.iter().copied().collect()
}

fn remote() -> HashMap<u64, u64> {
    map(&[
        (2330359, 1),
        (2378501, 2),
        (2378503, 3),
        (2378505, 5),
        (2378507, 7),
        (1086944, 44),
        (9999, 99),
        (9998, 98),
        (7001, 71),
    ])
}

#[test]
fn german_to_english_swaps_only_language_depots() {
    let vdf = rows();
    let rows = vdf.as_obj().unwrap();
    let installed = map(&[(2330359, 1), (2378501, 2), (2378505, 5)]);
    let plan = plan_language_switch(rows, 1086940, &installed, &remote(), "English").unwrap();
    assert_eq!(plan.remove, vec![(2378505, 5)]);
    assert_eq!(plan.add, vec![(2378503, 3)]);
    assert_eq!(plan.keep, vec![(2330359, 1), (2378501, 2)]);
}

#[test]
fn other_platform_and_dlc_depots_are_ignored() {
    let vdf = rows();
    let rows = vdf.as_obj().unwrap();
    let installed = map(&[(2330359, 1), (2378503, 3)]);
    let plan = plan_language_switch(rows, 1086940, &installed, &remote(), "german").unwrap();
    assert_eq!(plan.add, vec![(2378505, 5)]);
    assert_eq!(plan.remove, vec![(2378503, 3)]);
}

#[test]
fn installed_dlc_language_depots_are_swapped() {
    let vdf = rows();
    let rows = vdf.as_obj().unwrap();
    let installed = map(&[(2330359, 1), (2378505, 5), (9999, 99)]);
    let plan = plan_language_switch(rows, 1086940, &installed, &remote(), "english").unwrap();
    assert_eq!(plan.remove, vec![(9999, 99), (2378505, 5)]);
    assert_eq!(plan.add, vec![(9998, 98), (2378503, 3)]);
    assert_eq!(plan.keep, vec![(2330359, 1)]);
    assert_eq!(plan.dlc, [(9998, 5555)].into_iter().collect());

    // And back again.
    let installed = map(&[(2330359, 1), (2378503, 3), (9998, 98)]);
    let plan = plan_language_switch(rows, 1086940, &installed, &remote(), "german").unwrap();
    assert_eq!(plan.remove, vec![(9998, 98), (2378503, 3)]);
    assert_eq!(plan.add, vec![(9999, 99), (2378505, 5)]);
}

#[test]
fn language_depot_without_replacement_stays() {
    let vdf = rows();
    let rows = vdf.as_obj().unwrap();
    // DLC 7000 ships German only.
    let installed = map(&[(2330359, 1), (2378505, 5), (7001, 71)]);
    let plan = plan_language_switch(rows, 1086940, &installed, &remote(), "french").unwrap();
    assert_eq!(plan.remove, vec![(2378505, 5)]);
    assert_eq!(plan.add, vec![(2378507, 7)]);
    assert_eq!(plan.keep, vec![(7001, 71), (2330359, 1)]);
    assert!(plan.dlc.is_empty());
}

#[test]
fn pure_language_depot_goes_without_english_depot() {
    // Half-Life: English lives in base depots.
    let vdf = steam_vdf_parser::parse_text(
        r#""depots" {
            "1" { "config" { "language" "" } }
            "8" { "config" { "oslist" "linux" } }
            "74" { "config" { "language" "german" } }
            "72" { "config" { "language" "french" } }
            "branches" { "public" { "buildid" "1" } }
        }"#,
    )
    .unwrap()
    .into_owned();
    let rows = vdf.as_obj().unwrap();
    let remote = map(&[(1, 1), (8, 8), (74, 74), (72, 72)]);
    let installed = map(&[(1, 1), (8, 8), (74, 74)]);
    let plan = plan_language_switch(rows, 70, &installed, &remote, "english").unwrap();
    assert_eq!(plan.remove, vec![(74, 74)]);
    assert!(plan.add.is_empty());
    assert_eq!(plan.keep, vec![(1, 1), (8, 8)]);
}

const MAC_ROWS: &str = r#""depots"
{
	"11" { "config" { "oslist" "macos" } }
	"12" { "config" { "oslist" "macos" "language" "english" } }
	"13" { "config" { "oslist" "macos" "language" "german" } }
	"21" { "config" { "oslist" "windows" } }
	"22" { "config" { "oslist" "windows" "language" "english" } }
	"23" { "config" { "oslist" "windows" "language" "german" } }
	"30" { "config" { "oslist" "windows,macos" } }
}
"#;

fn mac_rows() -> steam_vdf_parser::Vdf<'static> {
    steam_vdf_parser::parse_text(MAC_ROWS).unwrap().into_owned()
}

fn mac_remote() -> HashMap<u64, u64> {
    map(&[
        (11, 11),
        (12, 12),
        (13, 13),
        (21, 21),
        (22, 22),
        (23, 23),
        (30, 30),
    ])
}

#[test]
fn mac_install_swaps_mac_language_depots() {
    let vdf = mac_rows();
    let rows = vdf.as_obj().unwrap();
    let installed = map(&[(11, 11), (12, 12), (30, 30)]);
    let plan = plan_language_switch(rows, 10, &installed, &mac_remote(), "german").unwrap();
    assert_eq!(plan.remove, vec![(12, 12)]);
    assert_eq!(plan.add, vec![(13, 13)]);
    assert_eq!(plan.keep, vec![(11, 11), (30, 30)]);
}

#[test]
fn windows_install_with_shared_mac_depot_stays_windows() {
    let vdf = mac_rows();
    let rows = vdf.as_obj().unwrap();
    let installed = map(&[(21, 21), (22, 22), (30, 30)]);
    let plan = plan_language_switch(rows, 10, &installed, &mac_remote(), "german").unwrap();
    assert_eq!(plan.remove, vec![(22, 22)]);
    assert_eq!(plan.add, vec![(23, 23)]);
    assert_eq!(plan.keep, vec![(21, 21), (30, 30)]);
}

#[test]
fn unknown_language_lists_available_ones() {
    let vdf = rows();
    let rows = vdf.as_obj().unwrap();
    let installed = map(&[(2330359, 1)]);
    let err = plan_language_switch(rows, 1086940, &installed, &remote(), "polish").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("no polish depots"), "{msg}");
    assert!(msg.contains("german") && msg.contains("french"), "{msg}");
}

#[test]
fn shared_files_are_not_deleted() {
    let removed = vec![vec![
        "Data/Localization/German.pak".into(),
        "Data/Shared.pak".into(),
    ]];
    let remaining = vec![vec!["data/shared.pak".into(), "bin/game".into()]];
    assert_eq!(
        files_to_delete(&removed, &remaining),
        vec!["Data/Localization/German.pak".to_string()]
    );
}

#[test]
fn delete_refuses_paths_outside_the_install() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    std::fs::create_dir_all(root.join("Data/Loc")).unwrap();
    std::fs::write(root.join("Data/Loc/German.pak"), "x").unwrap();
    std::fs::write(dir.path().join("outside"), "x").unwrap();
    let files = vec!["Data/Loc/German.pak".to_string(), "../outside".to_string()];
    assert_eq!(delete_files(&root, &files), 1);
    assert!(!root.join("Data").exists());
    assert!(root.exists());
    assert!(dir.path().join("outside").exists());
}

#[test]
fn manifest_swap_keeps_everything_else() {
    let raw = "\"AppState\"\n{\n\t\"appid\"\t\t\"1086940\"\n\t\"StateFlags\"\t\t\"4\"\n\
        \t\"SizeOnDisk\"\t\t\"61\"\n\
        \t\"InstalledDepots\"\n\t{\n\
        \t\t\"2330359\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"1\"\n\t\t\t\"size\"\t\t\"10\"\n\t\t}\n\
        \t\t\"2378505\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"5\"\n\t\t\t\"size\"\t\t\"50\"\n\t\t}\n\
        \t\t\"5000\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"8\"\n\t\t\t\"size\"\t\t\"1\"\n\t\t\t\"dlcappid\"\t\t\"5001\"\n\t\t}\n\
        \t}\n\
        \t\"UserConfig\"\n\t{\n\t\t\"language\"\t\t\"german\"\n\t\t\"DisabledDLC\"\t\t\"5002\"\n\t}\n\
        \t\"MountedConfig\"\n\t{\n\t\t\"language\"\t\t\"german\"\n\t}\n}\n";
    let remove: HashSet<u32> = [2378505].into_iter().collect();
    let out = super::super::super::manifests::switch_language_in_manifest(
        raw,
        &remove,
        &[(2378503, 3, 30), (9998, 9, 4)],
        &[(9998, 5555)].into_iter().collect(),
        "english",
    );
    let depots = acf::installed_depots(&out);
    assert_eq!(depots.get(&2330359), Some(&1));
    assert_eq!(depots.get(&2378503), Some(&3));
    assert_eq!(depots.get(&5000), Some(&8));
    assert!(!depots.contains_key(&2378505));
    assert!(out.contains("\"dlcappid\"\t\t\"5001\""));
    assert_eq!(depots.get(&9998), Some(&9));
    assert!(out.contains("\"9998\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"9\"\n\t\t\t\"size\"\t\t\"4\"\n\t\t\t\"dlcappid\"\t\t\"5555\""));
    // 10 + 30 + 1 + 4.
    assert!(out.contains("\"SizeOnDisk\"\t\t\"45\""), "{out}");
    assert!(out.contains("\"DisabledDLC\"\t\t\"5002\""));
    assert!(out.contains("\"StateFlags\"\t\t\"4\""));
    assert_eq!(
        super::super::super::manifests::user_config_language(&out).as_deref(),
        Some("english")
    );
    assert!(!out.contains("german"));
}

#[test]
fn manifest_swap_adds_missing_user_config() {
    let raw = "\"AppState\"\n{\n\t\"appid\"\t\t\"1\"\n}\n";
    let out = super::super::super::manifests::switch_language_in_manifest(
        raw,
        &HashSet::new(),
        &[],
        &HashMap::new(),
        "german",
    );
    assert_eq!(
        super::super::super::manifests::user_config_language(&out).as_deref(),
        Some("german")
    );
}
