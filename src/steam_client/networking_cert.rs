//! SDR networking certificates; keys in memory.
use super::*;
use std::collections::HashMap;
use std::io::{Read, Write};

use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use steam_vent_proto::enums_clientserver::EMsg;
use steam_vent_proto::steammessages_clientserver::{
    CMsgClientNetworkingCertReply, CMsgClientNetworkingCertRequest,
};
use steam_vent_proto::steamnetworkingsockets_messages_certs::{
    CMsgSteamDatagramCertificate, CMsgSteamDatagramCertificateRequest,
    CMsgSteamDatagramCertificateSigned, cmsg_steam_datagram_certificate::EKeyType,
};
use steam_vent_proto_common::{RpcMessage, RpcMessageWithKind};
use zeroize::Zeroizing;

/// Renew this long before expiry.
pub const RENEW_BEFORE: u32 = 3 * 3600;
/// Gap after a failed request.
pub const RETRY_AFTER: u32 = 30;
/// CM job timeout.
const JOB_TIMEOUT: Duration = Duration::from_secs(15);
/// Selects the `key_data` format.
pub const KEY_FORMAT_ENV: &str = "AURELIA_SDR_CERT_KEY_FORMAT";

/// What `key_data` carries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KeyFormat {
    /// Raw 32-byte key (SteamKit).
    #[default]
    Raw,
    /// `CMsgSteamDatagramCertificateRequest` (GNS).
    Request,
}

impl KeyFormat {
    /// Parses `raw` or `request`.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "raw" => Some(Self::Raw),
            "request" => Some(Self::Request),
            _ => None,
        }
    }

    /// From the environment, else raw.
    pub fn from_env() -> Self {
        let value = std::env::var(KEY_FORMAT_ENV).unwrap_or_default();
        Self::parse(&value).unwrap_or_else(|| {
            tracing::warn!("{KEY_FORMAT_ENV}={value:?} unknown; using raw");
            Self::Raw
        })
    }
}

/// One Ed25519 key pair.
pub struct NetworkingKey {
    seed: Zeroizing<[u8; 32]>,
    public: [u8; 32],
}

impl std::fmt::Debug for NetworkingKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material.
        f.write_str("NetworkingKey(..)")
    }
}

impl NetworkingKey {
    /// Fresh random key.
    pub fn generate() -> Result<Self> {
        let mut seed = Zeroizing::new([0u8; 32]);
        SystemRandom::new()
            .fill(seed.as_mut())
            .map_err(|_| anyhow!("no system randomness for a networking key"))?;
        Self::from_seed(seed)
    }

    /// Key from a 32-byte seed.
    pub fn from_seed(seed: Zeroizing<[u8; 32]>) -> Result<Self> {
        let pair = Ed25519KeyPair::from_seed_unchecked(seed.as_ref())
            .map_err(|e| anyhow!("bad networking key: {e}"))?;
        let public = pair
            .public_key()
            .as_ref()
            .try_into()
            .map_err(|_| anyhow!("Ed25519 public key is not 32 bytes"))?;
        Ok(Self { seed, public })
    }

    pub fn public(&self) -> &[u8; 32] {
        &self.public
    }

    /// `key_data` for the CM request.
    pub fn key_data(&self, format: KeyFormat, steam_id: u64) -> Vec<u8> {
        match format {
            KeyFormat::Raw => self.public.to_vec(),
            KeyFormat::Request => {
                let mut cert = CMsgSteamDatagramCertificate::new();
                cert.set_key_type(EKeyType::ED25519);
                cert.set_key_data(self.public.to_vec());
                cert.set_identity_string(format!("steamid:{steam_id}"));
                let mut request = CMsgSteamDatagramCertificateRequest::new();
                request.cert = Some(cert).into();
                request.write_to_bytes().unwrap_or_default()
            }
        }
    }
}

/// A certificate Steam signed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkingCert {
    /// Serialized `CMsgSteamDatagramCertificate`.
    pub cert: Vec<u8>,
    pub ca_key_id: u64,
    pub ca_signature: Vec<u8>,
    pub created: u32,
    pub expiry: u32,
    pub app_ids: Vec<u32>,
}

impl NetworkingCert {
    /// Checks Steam's reply against `key`.
    pub fn from_reply(
        reply: &CMsgClientNetworkingCertReply,
        key: &NetworkingKey,
        now: u32,
    ) -> Result<Self> {
        if reply.cert().is_empty() || reply.ca_signature().is_empty() {
            bail!("Steam issued no networking certificate");
        }
        let body = CMsgSteamDatagramCertificate::parse_from_bytes(reply.cert())
            .context("Steam's networking certificate does not parse")?;
        if body.key_type() != EKeyType::ED25519 || body.key_data() != key.public() {
            bail!("Steam's networking certificate is for another key");
        }
        if body.time_expiry() <= now {
            bail!("Steam's networking certificate is already expired");
        }
        Ok(Self {
            cert: reply.cert().to_vec(),
            ca_key_id: reply.ca_key_id(),
            ca_signature: reply.ca_signature().to_vec(),
            created: body.time_created(),
            expiry: body.time_expiry(),
            app_ids: body.app_ids.clone(),
        })
    }

    /// GNS `SetCertificate` input, with key.
    pub fn signed_with_key(&self, key: &NetworkingKey) -> Zeroizing<Vec<u8>> {
        let mut signed = CMsgSteamDatagramCertificateSigned::new();
        signed.set_cert(self.cert.clone());
        signed.set_ca_key_id(self.ca_key_id);
        signed.set_ca_signature(self.ca_signature.clone());
        signed.set_private_key_data(key.seed.to_vec());
        let bytes = Zeroizing::new(signed.write_to_bytes().unwrap_or_default());
        // Wipe the copy inside.
        if let Some(seed) = signed.private_key_data.as_mut() {
            zeroize::Zeroize::zeroize(seed);
        }
        bytes
    }

    fn fresh(&self, now: u32) -> bool {
        self.expiry.saturating_sub(now) > RENEW_BEFORE
    }

    fn valid(&self, now: u32) -> bool {
        self.expiry > now
    }
}

/// What a game gets.
pub struct CertGrant {
    pub expiry: u32,
    /// Holds the private key.
    pub signed: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for CertGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertGrant")
            .field("expiry", &self.expiry)
            .finish_non_exhaustive()
    }
}

struct Entry {
    key: NetworkingKey,
    cert: Option<NetworkingCert>,
    failed_at: Option<u32>,
}

/// Keys and certs per account, app.
#[derive(Default)]
pub struct CertCache {
    entries: HashMap<(u64, u32), Entry>,
}

impl CertCache {
    /// Cached or freshly signed certificate.
    pub async fn get<F, Fut>(
        &mut self,
        steam_id: u64,
        app_id: u32,
        now: u32,
        format: KeyFormat,
        fetch: F,
    ) -> Result<CertGrant>
    where
        F: FnOnce(Vec<u8>) -> Fut,
        Fut: Future<Output = Result<CMsgClientNetworkingCertReply>>,
    {
        let entry = match self.entries.entry((steam_id, app_id)) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => v.insert(Entry {
                key: NetworkingKey::generate()?,
                cert: None,
                failed_at: None,
            }),
        };
        let grant = |entry: &Entry, cert: &NetworkingCert| CertGrant {
            expiry: cert.expiry,
            signed: cert.signed_with_key(&entry.key),
        };
        if let Some(cert) = entry.cert.as_ref().filter(|c| c.fresh(now)) {
            return Ok(grant(entry, cert));
        }
        let recently_failed = entry
            .failed_at
            .is_some_and(|t| now.saturating_sub(t) < RETRY_AFTER);
        if recently_failed {
            if let Some(cert) = entry.cert.as_ref().filter(|c| c.valid(now)) {
                return Ok(grant(entry, cert));
            }
            bail!("networking certificate request failed recently; retry later");
        }
        // Renewals keep the same key.
        let reply = fetch(entry.key.key_data(format, steam_id)).await;
        match reply.and_then(|r| NetworkingCert::from_reply(&r, &entry.key, now)) {
            Ok(cert) => {
                if !cert.app_ids.is_empty() && !cert.app_ids.contains(&app_id) {
                    tracing::warn!(app_id, apps = ?cert.app_ids, "networking cert names other apps");
                }
                tracing::info!(
                    app_id,
                    expiry = cert.expiry,
                    "steamworks: networking certificate issued"
                );
                entry.failed_at = None;
                let out = grant(entry, &cert);
                entry.cert = Some(cert);
                Ok(out)
            }
            Err(e) => {
                entry.failed_at = Some(now);
                match entry.cert.as_ref().filter(|c| c.valid(now)) {
                    Some(cert) => {
                        // Keep the old one, like GNS.
                        tracing::warn!(app_id, "networking cert renewal failed: {e:#}");
                        Ok(grant(entry, cert))
                    }
                    None => Err(e),
                }
            }
        }
    }

    /// Forget everything of `steam_id`.
    pub fn forget_account(&mut self, steam_id: u64) {
        self.entries.retain(|(id, _), _| *id != steam_id);
    }
}

/// Newtypes pin the CM message kinds.
#[derive(Debug, Default)]
struct CertRequest(CMsgClientNetworkingCertRequest);

#[derive(Debug, Default)]
struct CertReply(CMsgClientNetworkingCertReply);

macro_rules! kinded {
    ($t:ty, $inner:ty, $kind:expr) => {
        impl RpcMessage for $t {
            fn parse(reader: &mut dyn Read) -> ::protobuf::Result<Self> {
                <$inner as ::protobuf::Message>::parse_from_reader(reader).map(Self)
            }
            fn write(&self, writer: &mut dyn Write) -> ::protobuf::Result<()> {
                use ::protobuf::Message;
                self.0.write_to_writer(writer)
            }
            fn encode_size(&self) -> usize {
                use ::protobuf::Message;
                self.0.compute_size() as usize
            }
        }
        impl RpcMessageWithKind for $t {
            type KindEnum = EMsg;
            const KIND: Self::KindEnum = $kind;
        }
    };
}

kinded!(
    CertRequest,
    CMsgClientNetworkingCertRequest,
    EMsg::k_EMsgClientNetworkingCertRequest
);
kinded!(
    CertReply,
    CMsgClientNetworkingCertReply,
    EMsg::k_EMsgClientNetworkingCertRequestResponse
);

/// The CM request message.
pub fn cert_request(app_id: u32, key_data: Vec<u8>) -> CMsgClientNetworkingCertRequest {
    let mut request = CMsgClientNetworkingCertRequest::new();
    request.set_app_id(app_id);
    request.set_key_data(key_data);
    request
}

impl SteamClient {
    /// Asks the CM to sign `key_data`.
    pub async fn networking_cert(
        &self,
        app_id: u32,
        key_data: Vec<u8>,
    ) -> Result<CMsgClientNetworkingCertReply> {
        let connection = self.require_connection()?;
        let job = connection.job(CertRequest(cert_request(app_id, key_data)));
        let reply: CertReply = tokio::time::timeout(JOB_TIMEOUT, job)
            .await
            .map_err(|_| anyhow!("Steam did not answer the networking certificate request"))?
            .context("networking certificate request failed")?;
        Ok(reply.0)
    }
}

#[cfg(test)]
#[path = "networking_cert_tests.rs"]
mod tests;
