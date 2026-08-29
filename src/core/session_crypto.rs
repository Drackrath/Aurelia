//! Password-based encryption for `session.json`.
//!
//! The session file holds a long-lived Steam refresh token. With a session
//! password set (`aurelia config session-password`), the file is stored as a
//! ChaCha20-Poly1305 envelope whose key is derived from the password with
//! Argon2id, and is decrypted transparently ("on the fly") whenever the
//! session is loaded. The password comes from `AURELIA_SESSION_PASSWORD` or,
//! interactively, a one-time terminal prompt cached for the process.

use anyhow::{anyhow, bail, Context, Result};
use argon2::Argon2;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Marker so a plaintext session is never mistaken for an envelope.
const FORMAT: &str = "aurelia-session-v1";

/// Encrypted `session.json` on-disk form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedSession {
    /// Format tag; see [`FORMAT`].
    pub format: String,
    /// Plaintext display-name hint (persona, else account).
    ///
    /// Not secret (the refresh token is) and not authenticated —
    /// never trust it for anything but messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Hex Argon2id salt.
    pub salt: String,
    /// Hex ChaCha20-Poly1305 nonce.
    pub nonce: String,
    /// Hex ciphertext (tag included).
    pub ciphertext: String,
}

impl EncryptedSession {
    /// Parse only when the marker matches.
    pub fn from_json(raw: &str) -> Option<Self> {
        let env: Self = serde_json::from_str(raw).ok()?;
        (env.format == FORMAT).then_some(env)
    }
}

/// Argon2id key for this password + salt.
fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow!("session key derivation failed: {e}"))?;
    Ok(key)
}

/// Encrypt session plaintext with a password.
pub fn encrypt(plaintext: &[u8], password: &str) -> Result<EncryptedSession> {
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    rand::fill(&mut salt[..]);
    rand::fill(&mut nonce[..]);

    let key = derive_key(password, &salt)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .map_err(|e| anyhow!("session encryption failed: {e}"))?;

    Ok(EncryptedSession {
        format: FORMAT.to_string(),
        display_name: None,
        salt: hex::encode(salt),
        nonce: hex::encode(nonce),
        ciphertext: hex::encode(ciphertext),
    })
}

/// Decrypt an envelope with a password.
pub fn decrypt(envelope: &EncryptedSession, password: &str) -> Result<Vec<u8>> {
    let salt = hex::decode(&envelope.salt).context("bad salt in encrypted session")?;
    let nonce = hex::decode(&envelope.nonce).context("bad nonce in encrypted session")?;
    let ciphertext =
        hex::decode(&envelope.ciphertext).context("bad ciphertext in encrypted session")?;

    let key = derive_key(password, &salt)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    cipher
        .decrypt(Nonce::from_slice(&nonce), ciphertext.as_slice())
        .map_err(|_| {
            anyhow!("could not decrypt session.json — wrong session password?")
        })
}

/// Per-process password cache: prompt at most once.
static PASSWORD: Mutex<Option<String>> = Mutex::new(None);

/// Seed or replace the cache (e.g. after a password change).
pub fn cache_password(password: &str) {
    *PASSWORD.lock().unwrap() = Some(password.to_string());
}

/// Cached or env password; never prompts.
pub fn known_password() -> Option<String> {
    if let Some(p) = PASSWORD.lock().unwrap().clone() {
        return Some(p);
    }
    let p = std::env::var("AURELIA_SESSION_PASSWORD").ok()?;
    if p.is_empty() {
        return None;
    }
    cache_password(&p);
    Some(p)
}

/// The session password, resolved on first use.
///
/// Order: in-process cache, `AURELIA_SESSION_PASSWORD`, then an interactive
/// terminal prompt. Errors when no terminal is attached (e.g. the daemon) and
/// the environment variable is unset.
pub fn session_password() -> Result<String> {
    if let Some(p) = known_password() {
        return Ok(p);
    }
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        let p = rpassword::prompt_password("Session password: ")
            .context("failed reading session password")?;
        cache_password(&p);
        return Ok(p);
    }
    bail!(
        "session.json is encrypted and no session password is available — run \
         `aurelia daemon unlock` to hand it to the daemon, or set \
         AURELIA_SESSION_PASSWORD"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
