use super::*;

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
