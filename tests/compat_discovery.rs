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

/// Modern unified layout (Proton 11+/GE/CachyOS): components live under
/// `files/lib/wine/<component>/<arch>`. Detection must recognise the arch-split
/// folders, not just the flat legacy dirs.
#[test]
fn test_unified_layout_detection() {
    use std::fs;
    use tempfile::tempdir;
    use aurelia::utils::{detect_runner_components, ComponentSource};

    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // DXVK in the arch-split unified layout.
    let dxvk_arch = root.join("files/lib/wine/dxvk/x86_64-windows");
    fs::create_dir_all(&dxvk_arch).unwrap();
    for dll in ["d3d11.dll", "dxgi.dll", "d3d9.dll", "d3d8.dll", "d3d10core.dll"] {
        fs::write(dxvk_arch.join(dll), "fake dll").unwrap();
    }

    // VKD3D-Proton likewise.
    let vkd3d_arch = root.join("files/lib/wine/vkd3d-proton/x86_64-windows");
    fs::create_dir_all(&vkd3d_arch).unwrap();
    for dll in ["d3d12.dll", "d3d12core.dll"] {
        fs::write(vkd3d_arch.join(dll), "fake dll").unwrap();
    }

    let comps = detect_runner_components(root, None);
    let dxvk = comps.dxvk.expect("unified-layout DXVK should be detected");
    assert_eq!(dxvk.source, ComponentSource::BundledWithRunner);
    let vkd3d_proton = comps
        .vkd3d_proton
        .expect("unified-layout VKD3D-Proton should be detected");
    assert_eq!(vkd3d_proton.source, ComponentSource::BundledWithRunner);
}

/// Arch-specific resolution: a 32-bit game must never resolve a 64-bit component
/// directory — neither the `x86_64-windows` arch subdir nor a legacy `lib64` dir
/// (which carries no `windows` marker and used to leak through the bitness filter).
#[test]
fn test_arch_specific_resolution_excludes_64bit_for_32bit_game() {
    use std::fs;
    use tempfile::tempdir;
    use aurelia::launch::dll_provider_resolver::{DllProvider, DllProviderResolver};
    use aurelia::models::{D3D12ProviderPolicy, ExecutableArchitecture};
    use aurelia::utils::{ComponentInfo, ComponentSource, RunnerComponents};

    let tmp = tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    // 64-bit (unified arch dir + legacy lib64) and 32-bit DXVK d3d11.dll.
    let dir64 = root.join("files/lib/wine/dxvk/x86_64-windows");
    let dir32 = root.join("files/lib/wine/dxvk/i386-windows");
    let dir_lib64 = root.join("lib64/wine/dxvk");
    fs::create_dir_all(&dir64).unwrap();
    fs::create_dir_all(&dir32).unwrap();
    fs::create_dir_all(&dir_lib64).unwrap();
    fs::write(dir64.join("d3d11.dll"), "64").unwrap();
    fs::write(dir32.join("d3d11.dll"), "32").unwrap();
    fs::write(dir_lib64.join("d3d11.dll"), "lib64").unwrap();

    let components = RunnerComponents {
        dxvk: Some(ComponentInfo {
            version: "test".into(),
            source: ComponentSource::BundledWithRunner,
            path: None,
        }),
        ..Default::default()
    };

    let resolver = DllProviderResolver::new();
    let game_dir = root.join("no_such_game_dir");

    // 32-bit game: must land on the i386 dir, never x86_64 or lib64.
    let (res32, _) = resolver.resolve(
        &game_dir,
        &root,
        &components,
        &D3D12ProviderPolicy::Auto,
        &ExecutableArchitecture::X86,
        None,
        None,
        None,
    );
    let d3d11 = res32.iter().find(|r| r.name == "d3d11").unwrap();
    assert_eq!(d3d11.chosen_provider, DllProvider::Runner);
    let chosen = d3d11.chosen_path.as_ref().unwrap().to_string_lossy().replace('\\', "/");
    assert!(chosen.contains("i386-windows"), "expected i386 dir, got {chosen}");
    assert!(!chosen.contains("x86_64"), "32-bit game leaked into x86_64 dir: {chosen}");
    assert!(!chosen.contains("lib64"), "32-bit game leaked into lib64 dir: {chosen}");

    // 64-bit game: must never resolve the i386 dir.
    let (res64, _) = resolver.resolve(
        &game_dir,
        &root,
        &components,
        &D3D12ProviderPolicy::Auto,
        &ExecutableArchitecture::X86_64,
        None,
        None,
        None,
    );
    let d3d11_64 = res64.iter().find(|r| r.name == "d3d11").unwrap();
    assert_eq!(d3d11_64.chosen_provider, DllProvider::Runner);
    let chosen64 = d3d11_64.chosen_path.as_ref().unwrap().to_string_lossy().replace('\\', "/");
    assert!(!chosen64.contains("i386"), "64-bit game leaked into i386 dir: {chosen64}");
}
