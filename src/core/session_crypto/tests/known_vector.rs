use super::super::*;

// Written by argon2 0.5 / chacha20poly1305 0.10.
#[test]
fn decrypts_envelopes_from_older_releases() {
    let env = EncryptedSession {
        format: FORMAT.to_string(),
        display_name: None,
        keyring: false,
        salt: "63daf2b3a8d1e0f16272a0d017c3c76e".into(),
        nonce: "46e05c6237d29e48971f8b28".into(),
        ciphertext: "dfa916e84a7eac822ea6eee8388f947662282f052dabee3f3c106d8843e0ce63c742bf7b0bde8962ba9974f532f797b85d91".into(),
    };
    let plain = decrypt(&env, "fixture-pass").unwrap();
    assert_eq!(plain, b"{\"refresh_token\":\"fixture-secret\"}");
    assert!(decrypt(&env, "wrong").is_err());
}
