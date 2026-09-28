use super::is_safe_dir_name;

#[test]
fn accepts_plain_names() {
    for name in ["Game A", "Half-Life 2", "GE-Proton9-20", "Proton 9.0", ".hidden"] {
        assert!(is_safe_dir_name(name), "{name:?}");
    }
}

#[test]
fn rejects_paths_and_dot_names() {
    for name in ["", " ", ".", "..", "/root", "a/b", "a\\b", "C:\\Games", "../x", "a/"] {
        assert!(!is_safe_dir_name(name), "{name:?}");
    }
}

#[cfg(windows)]
#[test]
fn rejects_drive_prefix() {
    assert!(!is_safe_dir_name("C:"));
}
