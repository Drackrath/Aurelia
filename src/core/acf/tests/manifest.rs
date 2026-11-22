use super::super::*;

fn manifest(installdir: &str) -> String {
    format!("\"AppState\"\n{{\n\t\"appid\"\t\t\"10\"\n\t\"installdir\"\t\t\"{installdir}\"\n}}\n")
}

#[test]
fn keeps_a_plain_installdir() {
    assert_eq!(
        parse_app_manifest(&manifest("Half-Life"))
            .install_dir
            .as_deref(),
        Some("Half-Life")
    );
}

#[test]
fn drops_installdirs_that_escape_common() {
    for bad in ["../x", "..", "a/../../b", "/etc", "a\\\\..\\\\b", " "] {
        let m = parse_app_manifest(&manifest(bad));
        assert_eq!(m.install_dir, None, "{bad:?}");
        assert_eq!(m.app_id, Some(10));
    }
}

#[test]
fn fully_installed_only_when_state_flag_set() {
    // StateFlags 4 = StateFullyInstalled.
    assert!(parse_app_manifest("\"AppState\"\n{\n\t\"StateFlags\"\t\t\"4\"\n}").fully_installed());
    // 6 = StateFullyInstalled | StateUpdateRequired (installed, update pending).
    assert!(parse_app_manifest("\"AppState\"\n{\n\t\"StateFlags\"\t\t\"6\"\n}").fully_installed());
    // 2 = StateUpdateRequired only: an install that started but never finished
    // (e.g. cancelled). Must NOT count as installed.
    assert!(!parse_app_manifest("\"AppState\"\n{\n\t\"StateFlags\"\t\t\"2\"\n}").fully_installed());
    // Missing StateFlags is treated as not installed.
    assert!(!parse_app_manifest("\"AppState\"\n{\n}").fully_installed());
}

#[test]
fn update_pending_when_update_required_flag_set() {
    // 4 = StateFullyInstalled only: up to date, no update pending.
    assert!(!parse_app_manifest("\"AppState\"\n{\n\t\"StateFlags\"\t\t\"4\"\n}").update_pending());
    // 6 = StateFullyInstalled | StateUpdateRequired: installed, update pending.
    assert!(parse_app_manifest("\"AppState\"\n{\n\t\"StateFlags\"\t\t\"6\"\n}").update_pending());
    // 1046 = a partially-started update
    assert!(
        parse_app_manifest("\"AppState\"\n{\n\t\"StateFlags\"\t\t\"1046\"\n}").update_pending()
    );
    // Missing StateFlags
    assert!(!parse_app_manifest("\"AppState\"\n{\n}").update_pending());
}

#[test]
fn acf_name_and_last_owner_parse() {
    let raw = "\"AppState\"\n{\n\t\"appid\"\t\t\"960090\"\n\t\"name\"\t\t\"Bloons TD 6\"\n\t\"LastOwner\"\t\t\"76561198000000001\"\n}\n";
    assert_eq!(
        parse_app_manifest(raw).display_name().as_deref(),
        Some("Bloons TD 6")
    );
    assert_eq!(parse_app_manifest(raw).last_owner, Some(76561198000000001));
    assert_eq!(parse_app_manifest("\"LastOwner\"\t\"0\"").last_owner, None);
    assert_eq!(parse_app_manifest("\"name\"\t\"\"").display_name(), None);
}

#[test]
fn installed_depots_maps_depot_to_manifest() {
    let raw = "\"AppState\"\n{\n\t\"InstalledDepots\"\n\t{\n\
        \t\t\"11\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"7\"\n\t\t\t\"size\"\t\t\"1\"\n\t\t}\n\
        \t\t\"12\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"8\"\n\t\t}\n\
        \t}\n\t\"SharedDepots\"\n\t{\n\t\t\"13\"\t\t\"14\"\n\t}\n}\n";
    let depots = installed_depots(raw);
    assert_eq!(depots.len(), 2);
    assert_eq!(depots.get(&11), Some(&7));
    assert_eq!(depots.get(&12), Some(&8));
}

#[test]
fn rewrite_branch_replaces_or_adds_betakey() {
    let with = "\"AppState\"\n{\n\t\"UserConfig\"\n\t{\n\t\t\"betakey\"\t\t\"old\"\n\t}\n}";
    let out = rewrite_branch(with, "beta");
    assert_eq!(
        out,
        "\"AppState\"\n{\n\t\"UserConfig\"\n\t{\n\t\t\"BetaKey\"\t\t\"beta\"\n\t}\n}"
    );
    let without = "\"AppState\"\n{\n\t\"UserConfig\"\n\t{\n\t}\n}";
    let out = rewrite_branch(without, "beta");
    assert_eq!(parse_app_manifest(&out).active_branch, "beta");
    assert_eq!(out.matches("BetaKey").count(), 1);
}

#[test]
fn reads_size_on_disk() {
    let m = parse_app_manifest("\"AppState\"\n{\n\t\"SizeOnDisk\"\t\t\"1073741824\"\n}");
    assert_eq!(m.size_on_disk, 1_073_741_824);
    assert_eq!(parse_app_manifest("\"AppState\"\n{\n}").size_on_disk, 0);
}
