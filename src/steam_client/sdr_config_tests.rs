use super::*;

/// Shape seen in public clients.
const SAMPLE: &str = r#"{
    "revision": 1700000000,
    "pops": {"fra": {"desc": "Frankfurt", "relays": [{"ipv4": "192.0.2.1", "port_range": [27015, 27060]}]}},
    "certs": ["AAAA"],
    "p2p_share_ip": {"cn": 0, "default": 40, "ru": 20},
    "relay_public_key": "x",
    "typical_pings": [["fra", "ams", 7]],
    "success": true
}"#;

#[test]
fn parses_revision_and_policy() {
    let config = SdrConfig::parse(SAMPLE).unwrap();
    assert_eq!(config.revision, 1_700_000_000);
    assert_eq!(config.share_ip("RU"), Some(20));
    assert_eq!(config.share_ip("de"), Some(40));
    assert_eq!(config.share_ip("cn"), Some(0));
    assert_eq!(config.certs, vec!["AAAA".to_string()]);
}

#[test]
fn ice_policy_follows_share_ip() {
    let config = SdrConfig::parse(SAMPLE).unwrap();
    assert_eq!(config.ice_enable("de"), ice::PRIVATE | ice::PUBLIC);
    assert_eq!(config.ice_enable("cn"), ice::PRIVATE);
    // Unknown policy: private only.
    assert_eq!(SdrConfig::default().ice_enable("de"), ice::PRIVATE);
}

#[test]
fn refusals_and_garbage_fail() {
    assert!(SdrConfig::parse(r#"{"success": false}"#).is_err());
    assert!(SdrConfig::parse("<html>").is_err());
    assert_eq!(SdrConfig::parse("{}").unwrap(), SdrConfig::default());
}

#[test]
fn stun_list_is_trimmed() {
    // SAFETY: only this test touches it.
    unsafe { std::env::set_var(STUN_ENV, " a.example:3478 , ,b.example:3478") };
    assert_eq!(stun_servers(), "a.example:3478,b.example:3478");
    unsafe { std::env::remove_var(STUN_ENV) };
    assert_eq!(stun_servers(), "");
}
