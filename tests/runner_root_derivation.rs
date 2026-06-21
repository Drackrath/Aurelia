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

// The umu runner reuses `derive_runner_root` to compute `PROTONPATH` — umu wants the Proton
// *directory* (the runner root), not the `proton` script inside it. These cases assert that
// pointing the runner at either a Proton dir or its `proton` script yields the same root.
#[test]
fn test_derive_runner_root_for_umu_protonpath_from_dir() {
    let tmp = tempdir().unwrap();
    let proton_dir = tmp.path().join("GE-Proton9-20");
    fs::create_dir_all(&proton_dir).unwrap();

    // A directory (the shape umu's PROTONPATH expects) resolves to itself.
    let root = derive_runner_root(&proton_dir);
    assert_eq!(root, proton_dir);
}

#[test]
fn test_derive_runner_root_for_umu_protonpath_from_proton_script() {
    let tmp = tempdir().unwrap();
    let proton_dir = tmp.path().join("GE-Proton9-20");
    fs::create_dir_all(&proton_dir).unwrap();
    let proton_script = proton_dir.join("proton");
    fs::write(&proton_script, "dummy").unwrap();

    // Given the `proton` script, the derived root is its containing dir — the value umu's
    // PROTONPATH must receive.
    let root = derive_runner_root(&proton_script);
    assert_eq!(root, proton_dir);
}
