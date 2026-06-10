use aurelia::utils::build_dll_overrides;

#[test]
fn test_build_dll_overrides_baseline() {
    // Default case: no graphics layers, no overlay
    let overrides = build_dll_overrides(false, false, false, false, false, None, false);

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
    let overrides = build_dll_overrides(true, false, false, true, false, None, false);

    // DXVK keys should be present
    assert!(overrides.contains("d3d9=n,b"));
    assert!(overrides.contains("d3d11=n,b"));
    assert!(overrides.contains("dxgi=n,b"));

    // Overlay should be disabled
    assert!(overrides.contains("GameOverlayRenderer=n"));
}

#[test]
fn test_build_dll_overrides_vkd3d_active() {
    let overrides = build_dll_overrides(false, true, false, true, false, None, false);

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

    let overrides = build_dll_overrides(true, false, false, true, false, Some(tmp.path()), false);

    // d3d11 should be skipped because it exists locally
    assert!(!overrides.contains("d3d11=n,b"));
    // other dxvk keys should still be present
    assert!(overrides.contains("d3d9=n,b"));
}

#[test]
fn test_build_dll_overrides_strict_dxvk() {
    let overrides = build_dll_overrides(true, false, false, true, false, None, true);

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

    let overrides = build_dll_overrides(true, false, false, true, false, Some(tmp.path()), true);

    // In strict mode, even if d3d11.dll exists locally, we should still add the override
    // and it should be 'n' (native only)
    assert!(overrides.contains("d3d11=n"));
}
