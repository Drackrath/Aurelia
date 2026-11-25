use super::super::*;

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
fn installdir_read_from_config_section() {
    let raw = r#""105600"
{
  "common" { "name" "Terraria" }
  "config" { "installdir" "Terraria" }
}"#;
    let vdf = find_vdf_in_pics(raw.as_bytes()).expect("parse pics vdf");
    let section = pics_app_section(vdf.value());
    assert_eq!(pics_installdir(section).as_deref(), Some("Terraria"));
}

#[test]
fn installdir_missing_is_none() {
    let raw = r#""appinfo" { "common" { "name" "No Dir" } }"#;
    let vdf = find_vdf_in_pics(raw.as_bytes()).expect("parse pics vdf");
    assert_eq!(pics_installdir(pics_app_section(vdf.value())), None);
}

#[test]
fn windows_and_linux_selection_unchanged() {
    let cases = [
        (None, true, true),
        (Some("windows"), true, false),
        (Some("linux"), false, true),
        (Some("macos"), false, false),
        (Some("windows,linux"), true, true),
        (Some("Windows"), true, false),
        (Some("solaris"), true, true),
    ];
    for (oslist, windows, linux) in cases {
        assert_eq!(
            should_keep_depot(oslist, DepotPlatform::Windows),
            windows,
            "{oslist:?}"
        );
        assert_eq!(
            should_keep_depot(oslist, DepotPlatform::Linux),
            linux,
            "{oslist:?}"
        );
    }
}
