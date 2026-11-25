use super::super::*;

// Adding a variant breaks this match.
fn ordinal(platform: DepotPlatform) -> usize {
    match platform {
        DepotPlatform::Linux => 0,
        DepotPlatform::Windows => 1,
    }
}

#[test]
fn all_lists_every_platform_once() {
    let mut seen: Vec<usize> = DepotPlatform::ALL.iter().map(|p| ordinal(*p)).collect();
    seen.sort_unstable();
    assert_eq!(seen, (0..=1).collect::<Vec<_>>());
}
