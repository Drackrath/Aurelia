use super::parse_manifest_overrides;

#[test]
fn parallel_lists_pair_by_position() {
    let map = parse_manifest_overrides(&[10, 20], &["111".into(), "222".into()]).unwrap();
    assert_eq!(map.get(&10), Some(&111));
    assert_eq!(map.get(&20), Some(&222));
    assert_eq!(map.len(), 2);
}

#[test]
fn combined_form_carries_its_own_depot() {
    let map = parse_manifest_overrides(&[], &["10:111".into(), "20:222".into()]).unwrap();
    assert_eq!(map.get(&10), Some(&111));
    assert_eq!(map.get(&20), Some(&222));
}

#[test]
fn mixing_bare_and_combined_is_supported() {
    // One combined entry plus one parallel pair.
    let map = parse_manifest_overrides(&[20], &["10:111".into(), "222".into()]).unwrap();
    assert_eq!(map.get(&10), Some(&111));
    assert_eq!(map.get(&20), Some(&222));
}

#[test]
fn unequal_parallel_lists_are_rejected() {
    let err = parse_manifest_overrides(&[10, 20], &["111".into()]).unwrap_err();
    assert!(err.to_string().contains("equal numbers"), "got: {err}");
}

#[test]
fn empty_input_is_rejected() {
    let err = parse_manifest_overrides(&[], &[]).unwrap_err();
    assert!(err.to_string().contains("at least one"), "got: {err}");
}

#[test]
fn duplicate_depot_is_rejected() {
    let err =
        parse_manifest_overrides(&[10], &["10:111".into(), "222".into()]).unwrap_err();
    assert!(err.to_string().contains("more than once"), "got: {err}");
}

#[test]
fn non_numeric_ids_are_rejected() {
    assert!(parse_manifest_overrides(&[], &["abc".into()]).is_err());
    assert!(parse_manifest_overrides(&[], &["10:xyz".into()]).is_err());
}
