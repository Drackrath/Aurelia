use super::super::*;

#[test]
fn round_trip() {
    let env = encrypt(b"{\"refresh_token\":\"secret\"}", "hunter2").unwrap();
    assert_eq!(env.format, FORMAT);
    let plain = decrypt(&env, "hunter2").unwrap();
    assert_eq!(plain, b"{\"refresh_token\":\"secret\"}");
}

#[test]
fn wrong_password_fails() {
    let env = encrypt(b"data", "right").unwrap();
    assert!(decrypt(&env, "wrong").is_err());
}

#[test]
fn plaintext_is_not_an_envelope() {
    assert!(EncryptedSession::from_json("{\"account_name\":\"x\"}").is_none());
    let env = encrypt(b"data", "pw").unwrap();
    let raw = serde_json::to_string(&env).unwrap();
    assert!(EncryptedSession::from_json(&raw).is_some());
}

#[test]
fn unique_salts_and_nonces() {
    let a = encrypt(b"data", "pw").unwrap();
    let b = encrypt(b"data", "pw").unwrap();
    assert_ne!(a.salt, b.salt);
    assert_ne!(a.nonce, b.nonce);
    assert_ne!(a.ciphertext, b.ciphertext);
}
