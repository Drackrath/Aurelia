use std::fs;
use tempfile::tempdir;
use aurelia::launch::dll_provider_resolver::DllProviderResolver;
use aurelia::utils::RunnerComponents;
use aurelia::models::D3D12ProviderPolicy;

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
    components.dxvk = Some(aurelia::utils::ComponentInfo {
        version: "2.3".into(),
        source: aurelia::utils::ComponentSource::BundledWithRunner,
        path: None,
    });

    let resolver = DllProviderResolver::new();
    let (resolutions, report) = resolver.resolve(
        tmp.path(),
        &proton_script,
        &components,
        &D3D12ProviderPolicy::Auto,
        &aurelia::models::ExecutableArchitecture::X86_64,
        None,
        None,
        None,
    );

    let d3d11 = resolutions.iter().find(|r| r.name == "d3d11").unwrap();
    assert!(d3d11.candidates.iter().any(|c| c.provider == aurelia::launch::dll_provider_resolver::DllProvider::Runner && c.exists));

    assert!(report.scan_roots.iter().any(|r| r.to_string_lossy().contains("files/lib/wine/dxvk")));
    assert!(report.components_found.contains_key("dxvk"));
}
