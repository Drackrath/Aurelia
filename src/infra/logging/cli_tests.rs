use super::*;

#[test]
fn directives_escalate_with_verbosity() {
    assert!(default_directives(0).contains("steam_vent=warn"));
    assert!(default_directives(1).contains("steam_vent=info"));
    assert!(default_directives(2).contains("steam_vent=debug"));
    assert_eq!(default_directives(3), "trace");
    assert_eq!(default_directives(9), "trace");
}

#[test]
fn directives_are_valid_env_filters() {
    // Each directive string must parse as an EnvFilter or the CLI would
    // panic at startup.
    for v in 0..=3 {
        EnvFilter::new(default_directives(v));
    }
}
