use super::*;

#[test]
fn normalize_strips_punctuation_and_case() {
    assert_eq!(normalize_runner_name("Proton - Experimental"), "protonexperimental");
    assert_eq!(normalize_runner_name("Proton Experimental"), "protonexperimental");
    assert_eq!(normalize_runner_name("GE-Proton9-20"), "geproton920");
}

#[test]
fn exact_directory_name_resolves() {
    let tmp = tempfile::tempdir().unwrap();
    let common = tmp.path().join("steamapps/common/Proton 9.0");
    std::fs::create_dir_all(&common).unwrap();
    let got = resolve_runner("Proton 9.0", tmp.path());
    assert_eq!(got, common);
}

#[test]
fn fuzzy_resolves_experimental_to_dashed_dir() {
    // The legacy default `experimental` and curated `Proton Experimental` must
    // both find Steam's on-disk `Proton - Experimental` directory.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("steamapps/common/Proton - Experimental");
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(resolve_runner("experimental", tmp.path()), dir);
    assert_eq!(resolve_runner("Proton Experimental", tmp.path()), dir);
}

#[test]
fn unresolvable_name_falls_back_to_itself() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("steamapps/common")).unwrap();
    // A name that matches nothing on disk is returned as-is (caller errors clearly).
    let got = resolve_runner("NoSuchRuntimeXYZ", tmp.path());
    assert_eq!(got, std::path::Path::new("NoSuchRuntimeXYZ"));
}
