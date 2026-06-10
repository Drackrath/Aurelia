#[test]
fn test_version_parsing() {
    use aurelia::utils::parse_short_version;

    assert_eq!(parse_short_version("dxvk (v2.7.1-404-g0bf876eb)"), "2.7.1-404");
    assert_eq!(parse_short_version("vkd3d-proton (v2.11-1-g0bf876eb)"), "2.11-1");
    assert_eq!(parse_short_version("v2.3"), "2.3");
    assert_eq!(parse_short_version("2.3.1-dirty"), "2.3.1-dirty");
    assert_eq!(parse_short_version("dxvk (2.10)"), "2.10");
    assert_eq!(parse_short_version(""), "unknown");
}

#[test]
fn test_path_discovery_roots() {
    use std::fs;
    use tempfile::tempdir;
    use aurelia::utils::{detect_runner_components, ComponentSource};

    let tmp = tempdir().unwrap();
    let runner_root = tmp.path().to_path_buf();

    // 1. Proton style layout
    let dxvk_dir = runner_root.join("files/lib/wine/dxvk");
    fs::create_dir_all(&dxvk_dir).unwrap();
    fs::write(dxvk_dir.join("d3d11.dll"), "fake dll").unwrap();

    let share_dxvk = runner_root.join("files/share/dxvk");
    fs::create_dir_all(&share_dxvk).unwrap();
    fs::write(share_dxvk.join("version"), "dxvk (v2.3-g1234567)").unwrap();

    // 2. Critical vkd3d layout (files/lib/wine/vkd3d)
    let vkd3d_dir = runner_root.join("files/lib/wine/vkd3d");
    fs::create_dir_all(&vkd3d_dir).unwrap();
    fs::write(vkd3d_dir.join("libvkd3d-1.dll"), "fake dll").unwrap();

    let share_vkd3d = runner_root.join("files/share/vkd3d");
    fs::create_dir_all(&share_vkd3d).unwrap();
    fs::write(share_vkd3d.join("version"), "vkd3d (v1.10-gabcdef0)").unwrap();

    let components = detect_runner_components(&runner_root, None);

    assert!(components.dxvk.is_some());
    let dxvk = components.dxvk.unwrap();
    assert_eq!(dxvk.version, "2.3");
    assert_eq!(dxvk.source, ComponentSource::BundledWithRunner);

    assert!(components.vkd3d.is_some());
    let vkd3d = components.vkd3d.unwrap();
    assert_eq!(vkd3d.version, "1.10");
    assert_eq!(vkd3d.source, ComponentSource::BundledWithRunner);
}
