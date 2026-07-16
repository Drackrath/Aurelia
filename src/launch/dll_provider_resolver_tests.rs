use super::*;
use tempfile::tempdir;
use std::fs;

#[test]
fn test_dll_priority_game_local() {
    let tmp = tempdir().unwrap();
    let game_dir = tmp.path().to_path_buf();
    let d3d9_dll = game_dir.join("d3d9.dll");
    fs::write(&d3d9_dll, "local content").unwrap();

    let resolver = DllProviderResolver::new();
    let runner_path = Path::new("/tmp/fake_runner");
    let components = crate::core::utils::RunnerComponents::default();
    let d3d12_policy = crate::core::models::D3D12ProviderPolicy::Auto;
    let arch = crate::core::models::ExecutableArchitecture::X86_64;
    let (resolutions, _) = resolver.resolve(&game_dir, runner_path, &components, &d3d12_policy, &arch, None, None, None);

    let d3d9_res = resolutions.iter().find(|r| r.name == "d3d9").unwrap();
    assert_eq!(d3d9_res.chosen_provider, DllProvider::GameLocal);
    assert_eq!(d3d9_res.chosen_path.as_ref().unwrap(), &d3d9_dll);
}

#[test]
fn test_dll_priority_system_fallback() {
    // We can't easily test system paths because they are absolute and might not exist
    // But we can verify the logic correctly identifies 'None' when no tier matches.
    let tmp = tempdir().unwrap();
    let game_dir = tmp.path().to_path_buf();

    let resolver = DllProviderResolver::new();
    let runner_path = Path::new("/tmp/fake_runner");
    let components = crate::core::utils::RunnerComponents::default();
    let d3d12_policy = crate::core::models::D3D12ProviderPolicy::Auto;
    let arch = crate::core::models::ExecutableArchitecture::X86_64;
    let (resolutions, _) = resolver.resolve(&game_dir, runner_path, &components, &d3d12_policy, &arch, None, None, None);

    for res in resolutions {
        if res.chosen_provider == DllProvider::System {
            // OK if system has them
        } else {
            assert_eq!(res.chosen_provider, DllProvider::None);
        }
    }
}

#[test]
fn test_d3d12_provider_selection() {
    let tmp = tempdir().unwrap();
    let runner_root = tmp.path().to_path_buf();
    let proton_dir = runner_root.join("files/lib/wine/vkd3d-proton");
    let wine_dir = runner_root.join("files/lib/wine/vkd3d");
    fs::create_dir_all(&proton_dir).unwrap();
    fs::create_dir_all(&wine_dir).unwrap();

    let proton_dll = proton_dir.join("d3d12.dll");
    let wine_dll = wine_dir.join("d3d12.dll");
    fs::write(&proton_dll, "proton").unwrap();
    fs::write(&wine_dll, "wine").unwrap();

    let mut components = crate::core::utils::RunnerComponents::default();
    components.vkd3d_proton = Some(crate::core::utils::ComponentInfo {
        version: "2.10".into(),
        source: crate::core::utils::ComponentSource::BundledWithRunner,
        path: None,
    });
    components.vkd3d = Some(crate::core::utils::ComponentInfo {
        version: "1.8".into(),
        source: crate::core::utils::ComponentSource::BundledWithRunner,
        path: None,
    });

    let resolver = DllProviderResolver::new();
    let game_dir = Path::new("/tmp/game");
    let arch = crate::core::models::ExecutableArchitecture::X86_64;

    // Case 1: Auto (Prefer Proton)
    let (res, _) = resolver.resolve(game_dir, &runner_root, &components, &crate::core::models::D3D12ProviderPolicy::Auto, &arch, None, None, None);
    let d3d12 = res.iter().find(|r| r.name == "d3d12").unwrap();
    assert_eq!(d3d12.chosen_path.as_ref().unwrap(), &proton_dll);

    // Case 2: Explicit Wine
    let (res, _) = resolver.resolve(game_dir, &runner_root, &components, &crate::core::models::D3D12ProviderPolicy::Vkd3dWine, &arch, None, None, None);
    let d3d12 = res.iter().find(|r| r.name == "d3d12").unwrap();
    assert_eq!(d3d12.chosen_path.as_ref().unwrap(), &wine_dll);

    // Case 3: Explicit Proton
    let (res, _) = resolver.resolve(game_dir, &runner_root, &components, &crate::core::models::D3D12ProviderPolicy::Vkd3dProton, &arch, None, None, None);
    let d3d12 = res.iter().find(|r| r.name == "d3d12").unwrap();
    assert_eq!(d3d12.chosen_path.as_ref().unwrap(), &proton_dll);
}

#[test]
fn test_d3d8_coverage() {
    let resolver = DllProviderResolver::new();
    assert!(resolver.target_dlls.contains(&"d3d8".to_string()));
}

#[test]
fn test_fallback_reason_populated() {
    let resolver = DllProviderResolver::new();
    let tmp = tempdir().unwrap();
    let arch = crate::core::models::ExecutableArchitecture::X86_64;
    let (res, _) = resolver.resolve(tmp.path(), tmp.path(), &crate::core::utils::RunnerComponents::default(), &crate::core::models::D3D12ProviderPolicy::Auto, &arch, None, None, None);
    let d3d11 = res.iter().find(|r| r.name == "d3d11").unwrap();
    assert_eq!(d3d11.chosen_provider, DllProvider::None);
    assert!(d3d11.fallback_reason.is_some());
}
