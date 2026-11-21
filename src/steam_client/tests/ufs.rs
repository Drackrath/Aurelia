use super::super::*;

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

#[test]
fn vdf_flag_accepts_binary_int() {
    use steam_vdf_parser::Value;
    assert!(vdf_flag(&Value::I32(1)));
    assert!(!vdf_flag(&Value::I32(0)));
    assert!(vdf_flag(&Value::Str("1".into())));
    assert!(!vdf_flag(&Value::Str("0".into())));
}

#[test]
fn parses_ufs_root_overrides() {
    let raw = r#""2784470"
{
  "ufs"
  {
"savefiles"
{
  "0" { "root" "WinAppDataRoaming" "path" "Co/Game" "pattern" "*" }
}
"rootoverrides"
{
  "0"
  {
    "root" "WinAppDataRoaming"
    "os" "Linux"
    "oscompare" "="
    "useinstead" "LinuxXdgDataHome"
    "addpath" "co"
    "pathtransforms"
    {
      "0" { "find" "Co/" "replace" "co/" }
    }
  }
}
  }
}"#;
    let vdf = find_vdf_in_pics(raw.as_bytes()).expect("parse pics vdf");
    let cfg = ufs_config_from_section(pics_app_section(vdf.value()));
    assert_eq!(cfg.savefiles.len(), 1);
    assert_eq!(cfg.root_overrides.len(), 1);
    let o = &cfg.root_overrides[0];
    assert_eq!(o.root, "WinAppDataRoaming");
    assert_eq!(o.use_instead, "LinuxXdgDataHome");
    assert_eq!(o.add_path, "co");
    assert_eq!(o.path_transforms, vec![("Co/".into(), "co/".into())]);
    assert!(o.applies_to("linux") && !o.applies_to("windows"));
}
