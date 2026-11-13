use super::*;

#[test]
fn game_config_without_runner_defaults_to_auto() {
    // A config written before the `runner` field existed must still parse.
    let legacy = r#"{ "forced_proton_version": "GE-Proton9-20", "platform_preference": null }"#;
    let cfg: GameConfig = serde_json::from_str(legacy).unwrap();
    assert_eq!(cfg.runner, GameRunner::Auto);
    assert_eq!(cfg.forced_proton_version.as_deref(), Some("GE-Proton9-20"));
}

#[test]
fn game_runner_round_trips_as_lowercase() {
    let cfg = GameConfig {
        runner: GameRunner::Luxtorpeda,
        ..Default::default()
    };
    let json = serde_json::to_string(&cfg).unwrap();
    assert!(json.contains("\"luxtorpeda\""), "got: {json}");
    let back: GameConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.runner, GameRunner::Luxtorpeda);
}

#[test]
fn game_config_without_pin_fields_defaults_unpinned() {
    // A config written before pinning existed must still parse, unpinned.
    let legacy = r#"{ "forced_proton_version": null, "platform_preference": null }"#;
    let cfg: GameConfig = serde_json::from_str(legacy).unwrap();
    assert!(!cfg.pinned);
    assert!(cfg.pinned_manifests.is_empty());
}

#[test]
fn game_config_pin_round_trips_and_omits_empty() {
    // An unpinned config must not emit the pin fields (skip_serializing_if).
    let unpinned = GameConfig::default();
    let json = serde_json::to_string(&unpinned).unwrap();
    assert!(!json.contains("pinned_manifests"), "got: {json}");

    // A pinned config round-trips its depot→manifest map.
    let mut manifests = HashMap::new();
    manifests.insert(1234u32, 5678u64);
    let pinned = GameConfig {
        pinned: true,
        pinned_manifests: manifests.clone(),
        ..Default::default()
    };
    let json = serde_json::to_string(&pinned).unwrap();
    let back: GameConfig = serde_json::from_str(&json).unwrap();
    assert!(back.pinned);
    assert_eq!(back.pinned_manifests, manifests);
}

#[test]
fn launcher_config_without_luxtorpeda_flag_defaults_false() {
    // Minimal legacy config.json (pre-luxtorpeda) must load.
    let legacy = r#"{ "steam_library_path": "/x", "proton_version": "experimental",
        "enable_cloud_sync": true }"#;
    let cfg: LauncherConfig = serde_json::from_str(legacy).unwrap();
    assert!(!cfg.luxtorpeda_enabled);
}

#[test]
fn launcher_config_without_proxy_defaults_to_direct() {
    // A config written before the `proxy` field existed must still parse, direct.
    let legacy = r#"{ "steam_library_path": "/x", "proton_version": "experimental",
        "enable_cloud_sync": true }"#;
    let cfg: LauncherConfig = serde_json::from_str(legacy).unwrap();
    assert_eq!(cfg.proxy, ProxyConfig::default());
    assert!(cfg.proxy.url.is_none());
}

#[test]
fn proxy_config_omits_empty_and_round_trips() {
    // An empty proxy must not emit either field (skip_serializing_if).
    let json = serde_json::to_string(&ProxyConfig::default()).unwrap();
    assert!(!json.contains("url"), "got: {json}");
    assert!(!json.contains("no_proxy"), "got: {json}");

    // A populated proxy round-trips both fields.
    let proxy = ProxyConfig {
        url: Some("socks5://127.0.0.1:1080".to_string()),
        no_proxy: Some("localhost,.internal".to_string()),
    };
    let back: ProxyConfig = serde_json::from_str(&serde_json::to_string(&proxy).unwrap()).unwrap();
    assert_eq!(back, proxy);
}

#[test]
fn launcher_config_without_download_limit_is_unlimited() {
    let legacy = r#"{ "steam_library_path": "/x", "proton_version": "experimental",
        "enable_cloud_sync": true }"#;
    let cfg: LauncherConfig = serde_json::from_str(legacy).unwrap();
    assert_eq!(cfg.download_limit_kbit, 0);
    assert_eq!(cfg.download_limit_bytes(), 0);
}

#[test]
fn download_limit_converts_kbit_to_bytes() {
    assert_eq!(kbit_to_bytes(8000), 1_000_000);
    assert_eq!(kbit_to_bytes(1), 125);
    assert_eq!(kbit_to_bytes(u64::MAX), u64::MAX / 8);
}

#[test]
fn launcher_config_without_anticheat_flag_defaults_on() {
    let legacy = r#"{ "steam_library_path": "/x", "proton_version": "experimental",
        "enable_cloud_sync": true }"#;
    let cfg: LauncherConfig = serde_json::from_str(legacy).unwrap();
    assert!(cfg.anticheat_auto_install);
    assert!(LauncherConfig::default().anticheat_auto_install);
}

fn steam_root_with_login(base: &Path, name: &str, age_secs: u64) -> PathBuf {
    let root = base.join(name);
    std::fs::create_dir_all(root.join("config")).unwrap();
    let file = std::fs::File::create(root.join("config/loginusers.vdf")).unwrap();
    let when = std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
    file.set_modified(when).unwrap();
    root
}

#[test]
fn pick_steam_root_prefers_newest_login() {
    let tmp = tempfile::tempdir().unwrap();
    let native = steam_root_with_login(tmp.path(), "native", 3600);
    let flatpak = steam_root_with_login(tmp.path(), "flatpak", 10);
    assert_eq!(pick_steam_root(&[native, flatpak.clone()]), Some(flatpak));
}

#[test]
fn pick_steam_root_falls_back_to_first_existing() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("missing");
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    assert_eq!(pick_steam_root(&[missing.clone(), a.clone(), b]), Some(a));
    assert_eq!(pick_steam_root(&[missing]), None);
}

#[cfg(unix)]
#[test]
fn pick_steam_root_ignores_symlinked_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let real = steam_root_with_login(tmp.path(), "real", 10);
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let other = steam_root_with_login(tmp.path(), "other", 3600);
    assert_eq!(
        pick_steam_root(&[link.clone(), other.clone(), real.clone()]),
        Some(link.clone())
    );
    // Duplicate must not beat newer root.
    let older = steam_root_with_login(tmp.path(), "older", 7200);
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&older, &link).unwrap();
    assert_eq!(pick_steam_root(&[link, other.clone(), older]), Some(other));
}
