use super::super::*;

// Adding a variant breaks this match.
fn ordinal(platform: DepotPlatform) -> usize {
    match platform {
        DepotPlatform::Linux => 0,
        DepotPlatform::Windows => 1,
        DepotPlatform::Macos => 2,
    }
}

#[test]
fn all_lists_every_platform_once() {
    let mut seen: Vec<usize> = DepotPlatform::ALL.iter().map(|p| ordinal(*p)).collect();
    seen.sort_unstable();
    assert_eq!(seen, (0..=2).collect::<Vec<_>>());
}

#[test]
fn names_round_trip() {
    for platform in DepotPlatform::ALL {
        assert_eq!(DepotPlatform::from_name(platform.as_str()), Some(platform));
    }
    assert_eq!(DepotPlatform::from_name("solaris"), None);
    assert_eq!(DepotPlatform::from_name("Linux"), None);
    assert_eq!(DepotPlatform::from_name("MacOS"), None);
}
