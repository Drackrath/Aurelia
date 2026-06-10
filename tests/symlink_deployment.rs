use std::fs;
use tempfile::tempdir;
use aurelia::utils::{deploy_dll_symlinks, cleanup_dll_symlinks};
use aurelia::launch::dll_provider_resolver::{DllResolution, DllProvider};
use aurelia::models::ExecutableArchitecture;

#[test]
fn test_symlink_deployment_and_cleanup() {
    let tmp = tempdir().unwrap();
    let prefix = tmp.path().join("prefix");
    let system32 = prefix.join("drive_c/windows/system32");
    fs::create_dir_all(&system32).unwrap();

    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let d3d11_src = src_dir.join("d3d11.dll");
    fs::write(&d3d11_src, "fake dxvk").unwrap();

    let resolutions = vec![
        DllResolution {
            name: "d3d11".into(),
            chosen_provider: DllProvider::Runner,
            chosen_path: Some(d3d11_src.clone()),
            fallback_reason: None,
            candidates: vec![],
        }
    ];

    // Original builtin
    let d3d11_dest = system32.join("d3d11.dll");
    fs::write(&d3d11_dest, "wine builtin").unwrap();

    // Deploy
    let deployed = deploy_dll_symlinks(&prefix, &resolutions, &ExecutableArchitecture::X86_64).unwrap();
    assert_eq!(deployed.len(), 1);
    assert!(d3d11_dest.exists());

    let meta = fs::symlink_metadata(&d3d11_dest).unwrap();
    assert!(meta.file_type().is_symlink());

    let backup = system32.join("d3d11.dll.bak");
    assert!(backup.exists());
    assert_eq!(fs::read_to_string(&backup).unwrap(), "wine builtin");

    // Cleanup
    cleanup_dll_symlinks(&prefix).unwrap();
    assert!(d3d11_dest.exists());
    let meta2 = fs::symlink_metadata(&d3d11_dest).unwrap();
    assert!(!meta2.file_type().is_symlink());
    assert_eq!(fs::read_to_string(&d3d11_dest).unwrap(), "wine builtin");
    assert!(!backup.exists());
}

#[test]
fn test_symlink_deployment_dual_arch() {
    let tmp = tempdir().unwrap();
    let prefix = tmp.path().join("prefix");
    let system32 = prefix.join("drive_c/windows/system32");
    let syswow64 = prefix.join("drive_c/windows/syswow64");
    fs::create_dir_all(&system32).unwrap();
    fs::create_dir_all(&syswow64).unwrap();

    let src_root = tmp.path().join("runner");
    let x64_src_dir = src_root.join("x86_64-windows");
    let x86_src_dir = src_root.join("i386-windows");
    fs::create_dir_all(&x64_src_dir).unwrap();
    fs::create_dir_all(&x86_src_dir).unwrap();

    let d3d11_x64 = x64_src_dir.join("d3d11.dll");
    let d3d11_x86 = x86_src_dir.join("d3d11.dll");
    fs::write(&d3d11_x64, "x64").unwrap();
    fs::write(&d3d11_x86, "x86").unwrap();

    let resolutions = vec![
        DllResolution {
            name: "d3d11".into(),
            chosen_provider: DllProvider::Runner,
            chosen_path: Some(d3d11_x64.clone()),
            fallback_reason: None,
            candidates: vec![],
        }
    ];

    // Deploy starting from x64
    let deployed = deploy_dll_symlinks(&prefix, &resolutions, &ExecutableArchitecture::X86_64).unwrap();
    // Should deploy both x64 to system32 and x86 to syswow64 if sibling found
    assert_eq!(deployed.len(), 2);

    assert!(system32.join("d3d11.dll").exists());
    assert!(syswow64.join("d3d11.dll").exists());

    assert!(fs::symlink_metadata(system32.join("d3d11.dll")).unwrap().file_type().is_symlink());
    assert!(fs::symlink_metadata(syswow64.join("d3d11.dll")).unwrap().file_type().is_symlink());

    // Cleanup
    cleanup_dll_symlinks(&prefix).unwrap();
    assert!(!system32.join("d3d11.dll").exists()); // no backup existed, so it's gone (or should it stay gone? in this case yes because no backup)
    assert!(!syswow64.join("d3d11.dll").exists());
}
