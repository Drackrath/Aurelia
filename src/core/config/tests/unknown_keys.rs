use super::super::*;
use serde_json::{Value, json};

/// Run with a throwaway config dir.
fn in_config_dir<T>(body: impl std::future::Future<Output = T>) -> (T, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let _env =
        crate::test_support::EnvGuard::set(&[("AURELIA_CONFIG_DIR", Some(tmp.path().as_os_str()))]);
    let out = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(body);
    (out, tmp)
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Dev-branch keys main does not know.
fn dev_keys() -> Value {
    json!({
        "aureliaworks": true,
        "aureliaworks_path": "/opt/aureliaworks",
        "aureliaworks_steam_install": { "mode": "stub", "ids": [1, 2] },
    })
}

#[test]
fn launcher_config_keeps_unknown_keys_on_save() {
    let mut raw = json!({
        "steam_library_path": "/games",
        "proton_version": "experimental",
        "enable_cloud_sync": true,
        "preferred_launch_options": { "70": "-dev" },
        "proxy": { "url": "http://p:8080", "proxy_auth_mode": "ntlm" },
        "game_configs": {
            "10": {
                "forced_proton_version": null,
                "platform_preference": null,
                "pinned_manifests": { "11": 123 },
                "aureliaworks_override": "steamless"
            },
            "20": { "aureliaworks_only": 7 }
        }
    });
    raw.as_object_mut()
        .unwrap()
        .extend(dev_keys().as_object().unwrap().clone());

    let (_, tmp) = in_config_dir(async {
        let path = config_dir().unwrap().join("config.json");
        std::fs::write(&path, raw.to_string()).unwrap();
        let mut cfg = LauncherConfig::load().await.unwrap();
        assert_eq!(cfg.preferred_launch_options[&70], "-dev");
        assert_eq!(cfg.game_configs[&10].pinned_manifests[&11], 123);
        cfg.game_configs.get_mut(&10).unwrap().forced_proton_version = Some("GE".into());
        cfg.save().await.unwrap();
        let back = LauncherConfig::load().await.unwrap();
        assert_eq!(
            back.game_configs[&10].forced_proton_version.as_deref(),
            Some("GE")
        );
    });

    let saved = read(&tmp.path().join("config.json"));
    for (key, value) in dev_keys().as_object().unwrap() {
        assert_eq!(&saved[key], value, "{key}");
    }
    assert_eq!(saved["proxy"]["proxy_auth_mode"], "ntlm");
    assert_eq!(
        saved["game_configs"]["10"]["aureliaworks_override"],
        "steamless"
    );
    // Unknown-only entry survives too.
    assert_eq!(saved["game_configs"]["20"]["aureliaworks_only"], 7);
    assert_eq!(saved["game_configs"]["10"]["forced_proton_version"], "GE");
}

#[test]
fn user_apps_keep_unknown_keys_on_save() {
    let raw = json!({
        "10": {
            "launch_options": "-novid",
            "env_variables": { "MANGOHUD": "1" },
            "steam_launch_config": { "no_vr": true, "aureliaworks_ui": "min" },
            "graphics_layers": { "dxvk_enabled": true, "dxvk_hud": "fps" },
            "hidden": false,
            "favorite": false,
            "aureliaworks_runtime": { "pinned": "2026" }
        }
    });

    let (_, tmp) = in_config_dir(async {
        let path = config_dir().unwrap().join("user_apps.json");
        std::fs::write(&path, raw.to_string()).unwrap();
        let mut store = load_user_configs().await.unwrap();
        store.get_mut(&10).unwrap().launch_options = "-console".into();
        save_user_configs(&store).await.unwrap();
        let back = load_user_configs().await.unwrap();
        assert_eq!(back[&10].launch_options, "-console");
        assert!(back[&10].graphics_layers.dxvk_enabled);
    });

    let saved = read(&tmp.path().join("user_apps.json"));
    let entry = &saved["10"];
    assert_eq!(entry["aureliaworks_runtime"], json!({ "pinned": "2026" }));
    assert_eq!(entry["steam_launch_config"]["aureliaworks_ui"], "min");
    assert_eq!(entry["graphics_layers"]["dxvk_hud"], "fps");
    assert_eq!(entry["launch_options"], "-console");
}

#[test]
fn known_only_configs_gain_no_keys() {
    let json = serde_json::to_value(GameConfig::default()).unwrap();
    let keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
    assert!(keys.iter().all(|k| k != "extra"), "{keys:?}");
    let cfg: GameConfig = serde_json::from_value(json).unwrap();
    assert_eq!(cfg, GameConfig::default());
}
