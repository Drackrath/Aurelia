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
    let cfg = GameConfig { runner: GameRunner::Luxtorpeda, ..Default::default() };
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
    let pinned = GameConfig { pinned: true, pinned_manifests: manifests.clone(), ..Default::default() };
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

#[tokio::test]
async fn user_configs_set_and_clear_round_trip_through_save_load() {
    use crate::core::models::UserAppConfig;

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("user_apps.json");

    // A missing store loads empty.
    let empty = load_user_configs_from(&path).await.unwrap();
    assert!(empty.is_empty());

    // Set the CLI-reachable per-game fields and round-trip them.
    let mut ua = UserAppConfig::default();
    ua.launch_options = "-novid -ignoredifferentvideocard".to_string();
    ua.env_variables.insert("MANGOHUD".to_string(), "1".to_string());
    ua.graphics_layers.dxvk_enabled = true;
    ua.graphics_layers.graphics_backend_policy = crate::core::models::GraphicsBackendPolicy::DXVK;
    ua.graphics_layers.d3d12_policy = crate::core::models::D3D12ProviderPolicy::Vkd3dProton;
    ua.graphics_layers.nvapi_enabled = false;
    ua.steam_launch_config.no_overlay = false;
    ua.gpu_preference = Some("1".to_string());
    ua.hidden = true;
    ua.favorite = true;

    let mut store = UserConfigStore::new();
    store.insert(271590, ua);
    save_user_configs_to(&path, &store).await.unwrap();

    let back = load_user_configs_from(&path).await.unwrap();
    let ua = back.get(&271590).unwrap();
    assert_eq!(ua.launch_options, "-novid -ignoredifferentvideocard");
    assert_eq!(ua.env_variables.get("MANGOHUD").map(String::as_str), Some("1"));
    assert!(ua.graphics_layers.dxvk_enabled);
    assert_eq!(
        ua.graphics_layers.graphics_backend_policy,
        crate::core::models::GraphicsBackendPolicy::DXVK
    );
    assert_eq!(
        ua.graphics_layers.d3d12_policy,
        crate::core::models::D3D12ProviderPolicy::Vkd3dProton
    );
    assert!(!ua.graphics_layers.nvapi_enabled);
    assert!(!ua.steam_launch_config.no_overlay);
    assert_eq!(ua.gpu_preference.as_deref(), Some("1"));
    assert!(ua.hidden);
    assert!(ua.favorite);

    // Clear the fields back to defaults and round-trip again.
    let mut store = back;
    {
        let ua = store.get_mut(&271590).unwrap();
        ua.launch_options.clear();
        ua.env_variables.remove("MANGOHUD");
        ua.graphics_layers = Default::default();
        ua.steam_launch_config = Default::default();
        ua.gpu_preference = None;
        ua.hidden = false;
        ua.favorite = false;
    }
    save_user_configs_to(&path, &store).await.unwrap();

    let back = load_user_configs_from(&path).await.unwrap();
    let ua = back.get(&271590).unwrap();
    assert!(ua.launch_options.is_empty());
    assert!(ua.env_variables.is_empty());
    assert!(!ua.graphics_layers.dxvk_enabled);
    assert!(ua.graphics_layers.nvapi_enabled);
    assert!(ua.steam_launch_config.no_overlay);
    assert!(ua.gpu_preference.is_none());
    assert!(!ua.hidden);
    assert!(!ua.favorite);
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
