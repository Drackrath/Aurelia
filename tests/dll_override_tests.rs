use aurelia::core::utils::build_dll_overrides;

#[test]
fn test_build_dll_overrides_baseline() {
    // Default case: no graphics layers, no overlay
    let overrides = build_dll_overrides(false, false, false, false, false, None, false, false);

    // Essential Steam integration should be present
    assert!(overrides.contains("vstdlib_s=n"));
    assert!(overrides.contains("steamclient=n"));

    // Unsafe D3D/DXGI defaults should NOT be present
    assert!(!overrides.contains("d3d9=n,b"));
    assert!(!overrides.contains("d3d11=n,b"));
    assert!(!overrides.contains("dxgi=n,b"));
    assert!(!overrides.contains("d3d12=n,b"));

    // Overlay should be enabled (not overridden to 'n')
    assert!(!overrides.contains("GameOverlayRenderer=n"));
}

#[test]
fn test_build_dll_overrides_dxvk_active() {
    let overrides = build_dll_overrides(true, false, false, true, false, None, false, false);

    // DXVK keys should be present
    assert!(overrides.contains("d3d9=n,b"));
    assert!(overrides.contains("d3d11=n,b"));
    assert!(overrides.contains("dxgi=n,b"));

    // Overlay should be disabled
    assert!(overrides.contains("GameOverlayRenderer=n"));
}

#[test]
fn test_build_dll_overrides_vkd3d_active() {
    let overrides = build_dll_overrides(false, true, false, true, false, None, false, false);

    // VKD3D keys should be present
    assert!(overrides.contains("d3d12=n,b"));

    // DXVK keys should NOT be present
    assert!(!overrides.contains("d3d11=n,b"));
}

#[test]
fn test_build_dll_overrides_local_dll_skip() {
    let tmp = tempfile::tempdir().unwrap();
    let d3d11_path = tmp.path().join("d3d11.dll");
    std::fs::write(&d3d11_path, "fake dll").unwrap();

    let overrides = build_dll_overrides(true, false, false, true, false, Some(tmp.path()), false, false);

    // d3d11 should be skipped because it exists locally
    assert!(!overrides.contains("d3d11=n,b"));
    // other dxvk keys should still be present
    assert!(overrides.contains("d3d9=n,b"));
}

#[test]
fn test_build_dll_overrides_steam_enabled_omits_steam_overrides() {
    // With Steam integration enabled, Aurelia must NOT neutralise the Steam client
    // DLLs or disable lsteamclient — Proton's defaults handle them so Steamworks
    // (online features, Family-Sharing) can initialise.
    let overrides = build_dll_overrides(true, false, false, true, false, None, false, true);

    assert!(!overrides.contains("steamclient=n"));
    assert!(!overrides.contains("steam_api=n"));
    assert!(!overrides.contains("lsteamclient="));
    assert!(!overrides.contains("vstdlib_s=n"));

    // Graphics overrides still apply normally.
    assert!(overrides.contains("d3d11=n,b"));
}

#[test]
fn test_build_dll_overrides_strict_dxvk() {
    let overrides = build_dll_overrides(true, false, false, true, false, None, true, false);

    // DXVK keys should use 'n' (native only) in strict mode
    assert!(overrides.contains("d3d9=n"));
    assert!(overrides.contains("d3d11=n"));
    assert!(overrides.contains("dxgi=n"));
    assert!(overrides.contains("d3d8=n"));
    assert!(overrides.contains("d3d10core=n"));

    // They should NOT contain 'n,b'
    assert!(!overrides.contains("d3d9=n,b"));
    assert!(!overrides.contains("d3d11=n,b"));
}

#[test]
fn test_build_dll_overrides_strict_dxvk_ignores_local() {
    let tmp = tempfile::tempdir().unwrap();
    let d3d11_path = tmp.path().join("d3d11.dll");
    std::fs::write(&d3d11_path, "fake dll").unwrap();

    let overrides = build_dll_overrides(true, false, false, true, false, Some(tmp.path()), true, false);

    // In strict mode, even if d3d11.dll exists locally, we should still add the override
    // and it should be 'n' (native only)
    assert!(overrides.contains("d3d11=n"));
}

#[test]
fn test_build_dll_overrides_vkd3d_proton_forces_native_dxgi() {
    // vkd3d-proton requires DXVK's dxgi — without the paired override, wined3d's
    // dxgi wins and crash-loops on llvmpipe swapchains.
    let overrides = build_dll_overrides(false, true, false, false, false, None, false, false);
    assert!(overrides.contains("d3d12=n,b"));
    assert!(overrides.contains("dxgi=n,b"));

    // No duplicate dxgi entry when DXVK already emits one.
    let both = build_dll_overrides(true, true, false, false, false, None, false, false);
    assert_eq!(both.matches("dxgi=").count(), 1);
}

#[test]
fn test_build_dll_overrides_plain_vkd3d_leaves_dxgi_alone() {
    let overrides = build_dll_overrides(false, false, true, false, false, None, false, false);
    assert!(overrides.contains("d3d12=n,b"));
    assert!(!overrides.contains("dxgi="));
}

#[test]
fn test_build_dll_overrides_vkd3d_proton_skips_game_shipped_dxgi() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("dxgi.dll"), "fake dll").unwrap();

    let overrides = build_dll_overrides(false, true, false, false, false, Some(tmp.path()), false, false);
    assert!(overrides.contains("d3d12=n,b"));
    assert!(!overrides.contains("dxgi="));
}

#[test]
fn test_merge_dll_overrides_user_wins_per_dll() {
    use aurelia::core::utils::merge_dll_overrides;

    let merged = merge_dll_overrides("d3d11=n,b;dxgi=n,b;steamclient=n", "d3d11=b;winhttp=n,b");

    // User entry replaces the computed one for the same DLL...
    assert!(merged.contains("d3d11=b"));
    assert!(!merged.contains("d3d11=n,b"));
    // ...while untouched computed entries and new user entries both survive.
    assert!(merged.contains("dxgi=n,b"));
    assert!(merged.contains("steamclient=n"));
    assert!(merged.contains("winhttp=n,b"));
}

#[test]
fn test_merge_dll_overrides_edge_cases() {
    use aurelia::core::utils::merge_dll_overrides;

    assert_eq!(merge_dll_overrides("", "d3d9=n"), "d3d9=n");
    assert_eq!(merge_dll_overrides("d3d9=n", ""), "d3d9=n");
    // Keys match case-insensitively and with a .dll suffix.
    assert_eq!(merge_dll_overrides("d3d9=n,b", "D3D9.dll=b"), "D3D9.dll=b");
}

#[test]
fn test_normalize_dxgi_pairing_repairs_split_pair() {
    use aurelia::core::utils::normalize_dxgi_pairing;

    // Missing dxgi is appended with the trigger's mode.
    assert_eq!(normalize_dxgi_pairing("d3d11=n,b", false), "d3d11=n,b;dxgi=n,b");
    assert_eq!(normalize_dxgi_pairing("d3d9=n", false), "d3d9=n;dxgi=n");
    // A non-native dxgi (e.g. from a fixup) is repaired in place.
    assert_eq!(normalize_dxgi_pairing("d3d11=n,b;dxgi=b", false), "d3d11=n,b;dxgi=n,b");
    // Already-consistent strings pass through untouched.
    assert_eq!(normalize_dxgi_pairing("d3d11=n;dxgi=n", false), "d3d11=n;dxgi=n");
    assert_eq!(normalize_dxgi_pairing("d3d11=b;steamclient=n", false), "d3d11=b;steamclient=n");
}

#[test]
fn test_normalize_dxgi_pairing_respects_explicit_user_dxgi() {
    use aurelia::core::utils::normalize_dxgi_pairing;

    assert_eq!(normalize_dxgi_pairing("d3d11=n,b;dxgi=b", true), "d3d11=n,b;dxgi=b");
    assert_eq!(normalize_dxgi_pairing("d3d11=n,b", true), "d3d11=n,b");
}

#[test]
fn test_vkd3d_proton_policy_falls_back_to_plain_vkd3d() {
    use aurelia::core::models::{D3D12ProviderPolicy, ExecutableArchitecture};
    use aurelia::core::utils::{ComponentInfo, ComponentSource, RunnerComponents};
    use aurelia::launch::dll_provider_resolver::{DllProvider, DllProviderResolver};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let wine_dir = root.join("files/lib/wine/vkd3d");
    std::fs::create_dir_all(&wine_dir).unwrap();
    let wine_dll = wine_dir.join("d3d12.dll");
    std::fs::write(&wine_dll, "wine").unwrap();

    let components = RunnerComponents {
        vkd3d: Some(ComponentInfo {
            version: "1.10".into(),
            source: ComponentSource::BundledWithRunner,
            path: None,
        }),
        ..Default::default()
    };

    let resolver = DllProviderResolver::new();
    let (res, _) = resolver.resolve(
        &root.join("no_such_game_dir"),
        &root,
        &components,
        &D3D12ProviderPolicy::Vkd3dProton,
        &ExecutableArchitecture::X86_64,
        None,
        None,
        None,
    );

    let d3d12 = res.iter().find(|r| r.name == "d3d12").unwrap();
    assert_eq!(d3d12.chosen_provider, DllProvider::Runner);
    assert_eq!(d3d12.chosen_path.as_ref().unwrap(), &wine_dll);
    let reason = d3d12
        .fallback_reason
        .as_ref()
        .expect("explicit vkd3d-proton resolving plain vkd3d must record a fallback reason");
    assert!(reason.contains("vkd3d-proton"), "unexpected reason: {reason}");
}
