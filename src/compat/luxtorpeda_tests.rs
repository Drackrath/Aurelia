use super::*;

#[test]
fn parses_commandline_path() {
    let manifest = "\"manifest\"\n{\n  \"commandline\" \"/luxtorpeda %verb%\"\n}\n";
    assert_eq!(parse_commandline(manifest).as_deref(), Some("/luxtorpeda"));
}

#[test]
fn parses_commandline_extra_spacing() {
    let manifest = "\"commandline\"    \"/bin/luxtorpeda   %verb%\"";
    assert_eq!(parse_commandline(manifest).as_deref(), Some("/bin/luxtorpeda"));
}

#[test]
fn missing_commandline_is_none() {
    assert_eq!(parse_commandline("\"manifest\" { }"), None);
}

#[test]
fn selects_tarball_over_checksum() {
    // Mirrors the asset-picking logic in `latest_release` without the network.
    let assets = [
        ("luxtorpeda.tar.gz.sha256", false),
        ("luxtorpeda.tar.gz", true),
    ];
    let chosen = assets
        .iter()
        .find(|(name, _)| name.ends_with(".tar.gz") && !name.contains(".sha"));
    assert_eq!(chosen.map(|(_, ok)| *ok), Some(true));
}
