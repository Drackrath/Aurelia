//! OS-keyring-based encryption for `session.json`.
//!
//! The session file holds a long-lived Steam refresh token. It is stored as
//! a ChaCha20-Poly1305 envelope keyed by a random secret kept in the OS
//! keyring (Secret Service / Credential Manager / Keychain), created on
//! first login and read back transparently by both the CLI and the daemon.
//! No passwords, no prompts; without a reachable keyring the session stays
//! plaintext with owner-only permissions.

use anyhow::{anyhow, Context, Result};
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
    /// Keyed to the OS keyring, not a password.
    #[serde(default)]
    pub keyring: bool,
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
        keyring: false,
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
            anyhow!("could not decrypt session.json — the key does not match this file")
        })
}

/// Per-process keyring-key cache: one D-Bus round trip.
static KEYRING_KEY: Mutex<Option<String>> = Mutex::new(None);

/// Whether the OS keyring may be used at all.
fn keyring_enabled() -> bool {
    std::env::var_os("AURELIA_DISABLE_KEYRING").is_none_or(|v| v.is_empty())
}

/// The keyring entry holding the session key.
fn keyring_entry() -> keyring::Result<keyring::Entry> {
    keyring::Entry::new("aurelia", "session-key")
}

/// The stored keyring session key, if reachable.
///
/// Blocking (D-Bus / OS call) — call from sync code or `spawn_blocking`.
pub fn keyring_secret() -> Option<String> {
    if !keyring_enabled() {
        return None;
    }
    if let Some(k) = KEYRING_KEY.lock().unwrap().clone() {
        return Some(k);
    }
    match keyring_entry().and_then(|e| e.get_password()) {
        Ok(key) => {
            *KEYRING_KEY.lock().unwrap() = Some(key.clone());
            Some(key)
        }
        Err(keyring::Error::NoEntry) => None,
        Err(e) => {
            tracing::debug!("OS keyring unavailable: {e}");
            None
        }
    }
}

/// The keyring session key, created on first use.
///
/// Blocking (D-Bus / OS call) — call from sync code or `spawn_blocking`.
pub fn keyring_secret_or_create() -> Option<String> {
    if !keyring_enabled() {
        return None;
    }
    if let Some(key) = keyring_secret() {
        return Some(key);
    }
    let mut raw = [0u8; 32];
    rand::fill(&mut raw[..]);
    let key = hex::encode(raw);
    match keyring_entry().and_then(|e| e.set_password(&key)) {
        Ok(()) => {
            *KEYRING_KEY.lock().unwrap() = Some(key.clone());
            tracing::info!("created an OS keyring key for session.json");
            Some(key)
        }
        Err(e) => {
            tracing::warn!("could not store a session key in the OS keyring: {e}");
            None
        }
    }
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
