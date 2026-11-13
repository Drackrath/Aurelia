//! Offline cache of app ownership tickets.

use anyhow::{Result, bail};
use rsa::{BigUint, Pkcs1v15Sign, RsaPublicKey};
use sha1::{Digest, Sha1};
use std::path::PathBuf;

use crate::core::atomic::write_private;
use crate::core::config::data_dir;

/// Steam's public "system" RSA modulus.
const STEAM_SYSTEM_MODULUS: &str = "dfec1ad62c10662c17353a14b07c59117f9dd3d82b7ae3e015cd191e46e87b8774a2184631a9031479828ee945a24912a923687389cf69a1b16146bdc1bebfd6011bd881d4dc90fbfe4f527366cb9570d7c58eba1c7a3375a1623446bb60b78068fa13a77a8a374b9ec6f45d5f3a99f99ec43ae963a2bb881928e0e714c04289";
const STEAM_SYSTEM_EXPONENT: u32 = 17;
const SIGNATURE_LEN: usize = 128;
const HEADER_LEN: usize = 40;

/// Steam's ticket signing key.
pub fn steam_system_key() -> RsaPublicKey {
    let modulus = hex::decode(STEAM_SYSTEM_MODULUS).expect("valid modulus hex");
    RsaPublicKey::new(
        BigUint::from_bytes_be(&modulus),
        BigUint::from(STEAM_SYSTEM_EXPONENT),
    )
    .expect("valid Steam system key")
}

/// Fields of a signed ownership ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipTicket {
    pub steam_id: u64,
    pub app_id: u32,
    pub issued: u32,
    pub expires: u32,
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
}

/// Parses a ticket Steam signed.
pub fn parse_signed(bytes: &[u8], key: &RsaPublicKey) -> Option<OwnershipTicket> {
    if bytes.len() < HEADER_LEN + SIGNATURE_LEN {
        return None;
    }
    let body_len = usize::try_from(u32_at(bytes, 0)).ok()?;
    if body_len < HEADER_LEN || body_len.checked_add(SIGNATURE_LEN)? != bytes.len() {
        return None;
    }
    let (body, signature) = bytes.split_at(body_len);
    let digest = Sha1::digest(body);
    key.verify(Pkcs1v15Sign::new::<Sha1>(), &digest, signature)
        .ok()?;
    Some(OwnershipTicket {
        steam_id: u64::from_le_bytes(body[8..16].try_into().ok()?),
        app_id: u32_at(body, 16),
        issued: u32_at(body, 32),
        expires: u32_at(body, 36),
    })
}

/// Where a ticket request stands.
#[derive(Debug)]
pub enum TicketSource {
    /// Connected; Steam's answer to the request.
    Online {
        steam_id: u64,
        fetched: Result<Vec<u8>>,
    },
    /// Daemon has no Steam connection.
    Offline { steam_id: Option<u64> },
}

/// Per-account ticket files on disk.
pub struct TicketCache {
    dir: PathBuf,
    key: RsaPublicKey,
}

impl TicketCache {
    pub fn new(dir: PathBuf, key: RsaPublicKey) -> Self {
        Self { dir, key }
    }

    /// Cache under the Aurelia data dir.
    pub fn open() -> Result<Self> {
        Ok(Self::new(
            data_dir()?.join("app-tickets"),
            steam_system_key(),
        ))
    }

    fn path(&self, steam_id: u64, app_id: u32) -> PathBuf {
        self.dir
            .join(format!("appOwnershipTicket_{steam_id}_{app_id}.bin"))
    }

    /// Stores a verified ticket for reuse.
    pub fn store(&self, steam_id: u64, app_id: u32, ticket: &[u8]) -> Result<()> {
        let Some(parsed) = parse_signed(ticket, &self.key) else {
            bail!("ownership ticket for app {app_id} has no valid Steam signature");
        };
        if parsed.steam_id != steam_id || parsed.app_id != app_id {
            bail!(
                "ownership ticket is for account {} app {}, not {steam_id} app {app_id}",
                parsed.steam_id,
                parsed.app_id
            );
        }
        write_private(&self.path(steam_id, app_id), ticket)
    }

    /// Valid cached ticket, else None.
    pub fn load(&self, steam_id: u64, app_id: u32, now: u32) -> Option<Vec<u8>> {
        let path = self.path(steam_id, app_id);
        let bytes = std::fs::read(&path).ok()?;
        let parsed = parse_signed(&bytes, &self.key)?;
        if parsed.steam_id != steam_id || parsed.app_id != app_id {
            return None;
        }
        if parsed.expires <= now {
            let _ = std::fs::remove_file(&path);
            return None;
        }
        Some(bytes)
    }

    /// Steam's answer online, cache offline.
    pub fn resolve(&self, source: TicketSource, app_id: u32, now: u32) -> Result<Vec<u8>> {
        match source {
            TicketSource::Online { steam_id, fetched } => {
                let ticket = fetched?;
                if let Err(e) = self.store(steam_id, app_id, &ticket) {
                    tracing::warn!(appid = app_id, "not caching ownership ticket: {e:#}");
                }
                Ok(ticket)
            }
            TicketSource::Offline { steam_id: None } => {
                bail!("Steam is offline and the account is unknown; no ownership ticket")
            }
            TicketSource::Offline {
                steam_id: Some(steam_id),
            } => match self.load(steam_id, app_id, now) {
                Some(ticket) => Ok(ticket),
                None => bail!(
                    "Steam is offline and no unexpired cached ownership ticket exists for app {app_id}"
                ),
            },
        }
    }
}

/// Current time as Steam RTime32.
pub fn now_rtime() -> u32 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    u32::try_from(secs).unwrap_or(u32::MAX)
}

#[cfg(test)]
#[path = "ticket_cache_tests.rs"]
mod tests;
