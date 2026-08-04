use super::*;
use aurelia::core::models::{D3D12ProviderPolicy, GraphicsBackendPolicy, UserAppConfig};

#[test]
fn tuning_no_flags_changes_nothing() {
    let mut ua = UserAppConfig::default();
    let changed = apply_game_tuning(&mut ua, &GameTuningArgs::default()).unwrap();
    assert!(!changed);
}

#[test]
fn tuning_sets_and_clears_launch_options() {
    let mut ua = UserAppConfig::default();
    let t = GameTuningArgs {
        launch_options: Some("-novid -console".to_string()),
        ..Default::default()
    };
    assert!(apply_game_tuning(&mut ua, &t).unwrap());
    assert_eq!(ua.launch_options, "-novid -console");

    let t = GameTuningArgs { clear_launch_options: true, ..Default::default() };
    assert!(apply_game_tuning(&mut ua, &t).unwrap());
    assert!(ua.launch_options.is_empty());
}

#[test]
fn tuning_sets_and_unsets_env_vars() {
    let mut ua = UserAppConfig::default();
    let t = GameTuningArgs {
        env: vec!["MANGOHUD=1".to_string(), "DXVK_HUD=fps,gpuload".to_string()],
        ..Default::default()
    };
    assert!(apply_game_tuning(&mut ua, &t).unwrap());
    assert_eq!(ua.env_variables.get("MANGOHUD").map(String::as_str), Some("1"));
    // The value may itself contain `=`-free commas etc.; only the FIRST `=` splits.
    assert_eq!(ua.env_variables.get("DXVK_HUD").map(String::as_str), Some("fps,gpuload"));

    let t = GameTuningArgs { unset_env: vec!["MANGOHUD".to_string()], ..Default::default() };
    assert!(apply_game_tuning(&mut ua, &t).unwrap());
    assert!(!ua.env_variables.contains_key("MANGOHUD"));
    assert!(ua.env_variables.contains_key("DXVK_HUD"));

    // Unsetting a missing key is a no-op, not an error.
    let t = GameTuningArgs { unset_env: vec!["MISSING".to_string()], ..Default::default() };
    assert!(!apply_game_tuning(&mut ua, &t).unwrap());
}

#[test]
fn tuning_rejects_malformed_env() {
    let mut ua = UserAppConfig::default();
    let t = GameTuningArgs { env: vec!["NO_EQUALS".to_string()], ..Default::default() };
    assert!(apply_game_tuning(&mut ua, &t).is_err());
    let t = GameTuningArgs { env: vec!["=value".to_string()], ..Default::default() };
    assert!(apply_game_tuning(&mut ua, &t).is_err());
}

#[test]
fn tuning_graphics_toggles_on_off_default() {
    let mut ua = UserAppConfig::default();
    let t = GameTuningArgs { dxvk: Some(ToggleArg::On), nvapi: Some(ToggleArg::Off), ..Default::default() };
    assert!(apply_game_tuning(&mut ua, &t).unwrap());
    assert!(ua.graphics_layers.dxvk_enabled);
    assert!(!ua.graphics_layers.nvapi_enabled);

    // `default` resets to the built-in defaults: dxvk off, nvapi on.
    let t = GameTuningArgs {
        dxvk: Some(ToggleArg::Default),
        nvapi: Some(ToggleArg::Default),
        ..Default::default()
    };
    assert!(apply_game_tuning(&mut ua, &t).unwrap());
    assert!(!ua.graphics_layers.dxvk_enabled);
    assert!(ua.graphics_layers.nvapi_enabled);
}

#[test]
fn tuning_sets_backend_and_d3d12_policies() {
    let mut ua = UserAppConfig::default();
    let t = GameTuningArgs {
        backend: Some(BackendArg::Dxvk),
        d3d12: Some(D3D12Arg::Vkd3dProton),
        ..Default::default()
    };
    assert!(apply_game_tuning(&mut ua, &t).unwrap());
    assert_eq!(ua.graphics_layers.graphics_backend_policy, GraphicsBackendPolicy::DXVK);
    assert_eq!(ua.graphics_layers.d3d12_policy, D3D12ProviderPolicy::Vkd3dProton);

    let t = GameTuningArgs {
        backend: Some(BackendArg::Auto),
        d3d12: Some(D3D12Arg::Auto),
        ..Default::default()
    };
    assert!(apply_game_tuning(&mut ua, &t).unwrap());
    assert_eq!(ua.graphics_layers.graphics_backend_policy, GraphicsBackendPolicy::Auto);
    assert_eq!(ua.graphics_layers.d3d12_policy, D3D12ProviderPolicy::Auto);
}

#[test]
fn tuning_steam_helpers_read_positively_over_negative_storage() {
    let mut ua = UserAppConfig::default();
    // Defaults suppress everything (`no_* = true`).
    assert!(ua.steam_launch_config.no_overlay);

    let t = GameTuningArgs { overlay: Some(ToggleArg::On), browser: Some(ToggleArg::On), ..Default::default() };
    assert!(apply_game_tuning(&mut ua, &t).unwrap());
    assert!(!ua.steam_launch_config.no_overlay);
    assert!(!ua.steam_launch_config.no_browser);

    let t = GameTuningArgs { overlay: Some(ToggleArg::Off), ..Default::default() };
    assert!(apply_game_tuning(&mut ua, &t).unwrap());
    assert!(ua.steam_launch_config.no_overlay);

    // `default` restores the suppressed defaults.
    let t = GameTuningArgs { browser: Some(ToggleArg::Default), ..Default::default() };
    assert!(apply_game_tuning(&mut ua, &t).unwrap());
    assert!(ua.steam_launch_config.no_browser);
}

#[test]
fn tuning_hidden_favorite_and_gpu() {
    let mut ua = UserAppConfig::default();
    let t = GameTuningArgs {
        hidden: Some(OnOffArg::On),
        favorite: Some(OnOffArg::On),
        gpu: Some("1".to_string()),
        ..Default::default()
    };
    assert!(apply_game_tuning(&mut ua, &t).unwrap());
    assert!(ua.hidden);
    assert!(ua.favorite);
    assert_eq!(ua.gpu_preference.as_deref(), Some("1"));

    let t = GameTuningArgs {
        hidden: Some(OnOffArg::Off),
        favorite: Some(OnOffArg::Off),
        gpu: Some("default".to_string()),
        ..Default::default()
    };
    assert!(apply_game_tuning(&mut ua, &t).unwrap());
    assert!(!ua.hidden);
    assert!(!ua.favorite);
    assert!(ua.gpu_preference.is_none());
}
