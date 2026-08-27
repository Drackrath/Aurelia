use std::fs;
use tempfile::tempdir;
use aurelia::launch::dll_provider_resolver::DllProviderResolver;
use aurelia::launch::dll_provider_resolver::DllResolveRequest;
use aurelia::core::utils::RunnerComponents;
use aurelia::core::models::D3D12ProviderPolicy;

#[test]
fn test_dll_resolution_report_includes_runner_candidates() {
    let tmp = tempdir().unwrap();
    let runner_root = tmp.path().to_path_buf();

    let dxvk_dir = runner_root.join("files/lib/wine/dxvk");
    fs::create_dir_all(&dxvk_dir).unwrap();
    fs::write(dxvk_dir.join("d3d11.dll"), "fake").unwrap();

    let proton_script = runner_root.join("proton");
    fs::write(&proton_script, "dummy").unwrap();

    let mut components = RunnerComponents::default();
    components.dxvk = Some(aurelia::core::utils::ComponentInfo {
        version: "2.3".into(),
        source: aurelia::core::utils::ComponentSource::BundledWithRunner,
        path: None,
    });

    let resolver = DllProviderResolver::new();
    let (resolutions, report) = resolver.resolve(&DllResolveRequest {
            game_exe_dir: tmp.path(),
            runner_path: &proton_script,
            runner_components: &components,
            d3d12_policy: &D3D12ProviderPolicy::Auto,
            target_arch: &aurelia::core::models::ExecutableArchitecture::X86_64,
            custom_dxvk_path: None,
            custom_vkd3d_path: None,
            custom_vkd3d_proton_path: None,
    });

    let d3d11 = resolutions.iter().find(|r| r.name == "d3d11").unwrap();
    assert!(d3d11.candidates.iter().any(|c| c.provider == aurelia::launch::dll_provider_resolver::DllProvider::Runner && c.exists));

    assert!(report.scan_roots.iter().any(|r| r.to_string_lossy().contains("files/lib/wine/dxvk")));
    assert!(report.components_found.contains_key("dxvk"));
}
