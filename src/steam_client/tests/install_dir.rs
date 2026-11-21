use super::super::*;

#[test]
fn sanitize_install_dir_never_escapes_common() {
    assert_eq!(sanitize_install_dir("Half-Life 2"), "Half-Life 2");
    for name in ["", " ", ".", ".."] {
        assert_eq!(sanitize_install_dir(name), "_", "{name:?}");
    }
    assert_eq!(sanitize_install_dir("../x"), ".._x");
}

#[test]
fn missing_pics_installdir_fails_install() {
    let err = choose_install_dir(620, None, None).unwrap_err();
    assert!(err.to_string().contains("no install directory"), "{err}");
    // Placeholder folders are never reused.
    assert!(choose_install_dir(620, None, Some("App 620".into())).is_err());
    assert!(choose_install_dir(620, None, Some("app 620".into())).is_err());
}

#[test]
fn install_dir_prefers_existing_then_pics() {
    let pics = || Some("Portal 2".to_string());
    assert_eq!(choose_install_dir(620, pics(), None).unwrap(), "Portal 2");
    assert_eq!(
        choose_install_dir(620, pics(), Some("Portal2Old".into())).unwrap(),
        "Portal2Old"
    );
    // Real existing folder survives PICS outage.
    assert_eq!(
        choose_install_dir(620, None, Some("Portal 2".into())).unwrap(),
        "Portal 2"
    );
}
