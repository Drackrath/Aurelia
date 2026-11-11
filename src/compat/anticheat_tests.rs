use super::anticheat::{AntiCheat, detect, runtime_env};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[test]
fn no_anticheat_in_plain_game() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("Binaries/Win64")).unwrap();
    fs::write(tmp.path().join("Binaries/Win64/game.exe"), b"").unwrap();
    assert!(detect(tmp.path()).is_empty());
}

#[test]
fn detects_eac_folder_case_insensitive() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("EASYANTICHEAT")).unwrap();
    assert_eq!(detect(tmp.path()), vec![AntiCheat::EasyAntiCheat]);
}

#[test]
fn detects_eac_launcher_exe() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("start_protected_game.exe"), b"").unwrap();
    assert_eq!(detect(tmp.path()), vec![AntiCheat::EasyAntiCheat]);
}

#[test]
fn detects_nested_battleye() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("Game/Binaries/BattlEye")).unwrap();
    assert_eq!(detect(tmp.path()), vec![AntiCheat::BattlEye]);
}

#[test]
fn ignores_markers_beyond_depth() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("a/b/c/d/EasyAntiCheat")).unwrap();
    assert!(detect(tmp.path()).is_empty());
}

#[test]
fn file_named_like_folder_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("EasyAntiCheat"), b"").unwrap();
    assert!(detect(tmp.path()).is_empty());
}

#[test]
fn detects_both_in_stable_order() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("BattlEye")).unwrap();
    fs::create_dir_all(tmp.path().join("EasyAntiCheat")).unwrap();
    assert_eq!(
        detect(tmp.path()),
        vec![AntiCheat::EasyAntiCheat, AntiCheat::BattlEye]
    );
}

#[test]
fn missing_install_dir_detects_nothing() {
    assert!(detect(std::path::Path::new("/nonexistent/aurelia/game")).is_empty());
}

#[test]
fn env_only_for_installed_runtimes() {
    let installed = HashMap::from([(1826330, PathBuf::from("/lib/eac"))]);
    let env = runtime_env(&[AntiCheat::EasyAntiCheat, AntiCheat::BattlEye], &installed);
    assert_eq!(
        env,
        vec![("PROTON_EAC_RUNTIME".to_string(), "/lib/eac".to_string())]
    );
}

#[test]
fn runtime_ids_and_vars() {
    assert_eq!(AntiCheat::EasyAntiCheat.runtime_app_id(), 1826330);
    assert_eq!(AntiCheat::BattlEye.runtime_app_id(), 1161040);
    assert_eq!(AntiCheat::BattlEye.env_var(), "PROTON_BATTLEYE_RUNTIME");
}

#[test]
fn detects_unreal_binaries_layout() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("Proj/Binaries/Win64/EasyAntiCheat")).unwrap();
    assert_eq!(detect(tmp.path()), vec![AntiCheat::EasyAntiCheat]);
}
