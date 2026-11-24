use super::super::prefix::check_same_wine;
use std::path::Path;

#[test]
fn same_wine_is_accepted() {
    let wine = Path::new("/runners/GE-Proton11-11/files/bin/wine");
    assert!(check_same_wine(wine, wine, "GE-Proton11-11", "GE-Proton11-11", 1).is_ok());
}

#[test]
fn symlinked_build_matches_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("GE-Proton11-11");
    std::fs::create_dir_all(real.join("files/bin")).unwrap();
    std::fs::write(real.join("files/bin/wine"), "").unwrap();
    let latest = dir.path().join("GE-Proton-latest");
    std::os::unix::fs::symlink(&real, &latest).unwrap();
    assert!(
        check_same_wine(
            &latest.join("files/bin/wine"),
            &real.join("files/bin/wine"),
            "GE-Proton-latest",
            "GE-Proton11-11",
            1,
        )
        .is_ok()
    );
}

#[test]
fn different_wine_is_refused_with_fix() {
    let err = check_same_wine(
        Path::new("/runners/GE-Proton11-11/files/bin/wine"),
        Path::new("/runners/Proton - Experimental/files/bin/wine"),
        "GE-Proton11-11",
        "experimental",
        2784470,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("version mismatch"), "{err}");
    assert!(err.contains("steam-runtime-runner GE-Proton11-11"), "{err}");
    assert!(
        err.contains("config game 2784470 --proton experimental"),
        "{err}"
    );
}
