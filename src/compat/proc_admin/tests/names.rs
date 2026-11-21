use super::super::*;

#[test]
fn recognises_aurelia_executable_names() {
    assert!(is_aurelia_name("aurelia"));
    assert!(is_aurelia_name("aurelia.exe"));
    assert!(is_aurelia_name("Aurelia.EXE"));
    assert!(!is_aurelia_name("aurelia-helper"));
    assert!(!is_aurelia_name("steam.exe"));
}
