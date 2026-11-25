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

    let launch_options =
        parse_launch_info_from_vdf(10, raw, DepotPlatform::Linux).expect("parse launch info");
    let launch = &launch_options[0];
    assert_eq!(launch.target, LaunchTarget::NativeLinux);
    assert_eq!(launch.executable, "linux/game.sh");
    assert_eq!(launch.arguments, "-foo -bar");
}

fn launch_vdf(entries: &str) -> String {
    format!(r#""appinfo" {{ "config" {{ "launch" {{ {entries} }} }} }}"#)
}

fn parsed_targets(vdf: &str, host: DepotPlatform) -> Vec<LaunchTarget> {
    parse_launch_info_from_vdf(10, vdf, host)
        .expect("parse launch info")
        .into_iter()
        .map(|o| o.target)
        .collect()
}

#[test]
fn untagged_entry_target_follows_the_host() {
    use LaunchTarget::{NativeLinux, WindowsProton};
    let vdf = launch_vdf(r#""0" { "executable" "game" } "1" { "executable" "game.exe" }"#);
    assert_eq!(
        parsed_targets(&vdf, DepotPlatform::Linux),
        [NativeLinux, WindowsProton]
    );
    assert_eq!(
        parsed_targets(&vdf, DepotPlatform::Windows),
        [WindowsProton, WindowsProton]
    );
    assert_eq!(
        parsed_targets(&vdf, DepotPlatform::Macos),
        [WindowsProton, WindowsProton]
    );
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

#[test]
fn macos_target_keeps_mac_and_shared_depots() {
    let mac = DepotPlatform::Macos;
    assert!(should_keep_depot(Some("macos"), mac));
    assert!(should_keep_depot(Some("MacOS"), mac));
    assert!(should_keep_depot(Some("windows,macos"), mac));
    assert!(should_keep_depot(None, mac));
    assert!(should_keep_depot(Some("solaris"), mac));
    assert!(!should_keep_depot(Some("windows"), mac));
    assert!(!should_keep_depot(Some("linux"), mac));
}

fn platforms_of(depots: &str) -> Vec<DepotPlatform> {
    let vdf = steam_vdf_parser::parse_text(depots).unwrap();
    available_platforms(vdf.as_obj())
}

#[test]
fn available_platforms_lists_mac_depots() {
    assert_eq!(
        platforms_of(
            r#""depots" {
                "1" { "config" { "oslist" "macos" } }
                "2" { "config" { "oslist" "windows" } }
            }"#
        ),
        vec![DepotPlatform::Windows, DepotPlatform::Macos]
    );
    assert_eq!(
        platforms_of(r#""depots" { "1" { "config" { "oslist" "macos" } } }"#),
        vec![DepotPlatform::Macos]
    );
    assert_eq!(
        platforms_of(r#""depots" { "1" { "config" { "language" "" } } }"#),
        vec![DepotPlatform::Windows]
    );
    assert_eq!(available_platforms(None), vec![DepotPlatform::Windows]);
}
