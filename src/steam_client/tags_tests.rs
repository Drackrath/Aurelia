use super::tags_table::{tag_name, TAGS};

#[test]
fn table_is_sorted_and_unique() {
    assert!(TAGS.len() > 400, "table looks empty: {}", TAGS.len());
    assert!(TAGS.windows(2).all(|w| w[0].id < w[1].id));
    assert!(TAGS.iter().all(|t| !t.name.is_empty()));
}

#[test]
fn known_ids_resolve() {
    assert_eq!(tag_name(19), Some("Action"));
    assert_eq!(tag_name(492), Some("Indie"));
    assert_eq!(tag_name(21978), Some("VR"));
    assert_eq!(tag_name(0), None);
    assert_eq!(tag_name(u32::MAX), None);
}
