use super::super::*;

#[test]
fn unit_tests_never_use_the_real_config_dir() {
    let _env = crate::test_support::EnvGuard::set(&[("AURELIA_CONFIG_DIR", None)]);
    let dir = config_dir().unwrap();
    assert!(dir.starts_with(std::env::temp_dir()), "{dir:?}");
    assert_ne!(dir, home_dir().unwrap().join(".config").join("Aurelia"));
    // Stable within one test process.
    assert_eq!(config_dir().unwrap(), dir);
}
