use aurelia::utils::derive_runner_root;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_derive_runner_root_from_wine_bin() {
    let tmp = tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let wine = bin_dir.join("wine");
    fs::write(&wine, "dummy").unwrap();

    let root = derive_runner_root(&wine);
    assert_eq!(root, tmp.path());
}

#[test]
fn test_derive_runner_root_from_proton_script() {
    let tmp = tempdir().unwrap();
    let proton = tmp.path().join("proton");
    fs::write(&proton, "dummy").unwrap();

    let root = derive_runner_root(&proton);
    assert_eq!(root, tmp.path());
}

#[test]
fn test_derive_runner_root_from_dir() {
    let tmp = tempdir().unwrap();
    let root = derive_runner_root(tmp.path());
    assert_eq!(root, tmp.path());
}
