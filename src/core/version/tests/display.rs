use super::super::*;

#[test]
fn display_is_number_then_codename() {
    assert_eq!(DISPLAY, format!("{NUMBER} {CODENAME}"));
}

#[test]
fn number_carries_no_codename() {
    assert_eq!(NUMBER, env!("CARGO_PKG_VERSION"));
    assert!(NUMBER.starts_with(|c: char| c.is_ascii_digit()));
    assert!(!NUMBER.contains(CODENAME));
}

#[test]
fn codename_is_plain_words() {
    assert!(!CODENAME.is_empty());
    assert_eq!(CODENAME, CODENAME.trim());
    // Digits would confuse version parsers.
    assert!(!CODENAME.chars().any(|c| c.is_ascii_digit()));
}
