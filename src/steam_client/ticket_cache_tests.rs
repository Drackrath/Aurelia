use super::*;
use anyhow::anyhow;
use rsa::RsaPrivateKey;
use rsa::pkcs1::DecodeRsaPrivateKey;

/// Test-only key, not Steam's.
const TEST_KEY: &str = "-----BEGIN RSA PRIVATE KEY-----
MIICXQIBAAKBgQD3IvYX6HMpPpU+j88g6hGilmauA/pNN8otjA9DolaQ6gDpYdnX
S3ndODCcYqdqNiMG7UnfZEQntzyblvUEXhJDHeut3oedVxHpvK+5LxbLa+ICqxP/
f28Bcc2cHsQx3TTj+LOQaznbXQ1s6HiYLdrwEGItET1RFMZ7fCByyhlGXQIDAQAB
AoGAIE1LiThyxPNPIow9cHnnsC6kYR5X+vvNy0JBRH/VfL95Owf1JzTQSt/rSqWC
aFmHHoKpYy+B0QccPuHuSJHbJPTjDMRDf8UhysrB++PU8kOwrO7OWb+RHozNGZtc
dB6i+fuChO19sLWAagT6CKwU7wqpJlWOsqUeAcvgmyj4PYECQQD/xhSgqP08OP5Z
uV9UjljuzwlqUlIaTwPxbxZD51wsMBHl1lV0GfERKMAhKGqrD3MTvWETbWySTeFx
ld2zswMxAkEA91rswzYR1KEYcAdtGwBJG552AsuVMK4Ayo5Eki3XF+l8lhtRzDDD
xKhCs/IbwWMYGvEnmXw095pnqFNDVUvy7QJATkr283rnEXEswpBQ0xRDus4bRrVu
NmlrfpMPkLeYalpIkD88py5tsV/09/FkPXqcMjWtKoI7UsmOMfO7Co/IcQJBAK3q
STQMDUdiKSuDnSvx7HU12iWm/HOotDe3BfF9t5FBgm6MMNwPoWXhJF0fKnKzeNxg
mIYrNZauApnwSRRs/RUCQQCrQRJ//zKTTHp9nVl+wbzjwhSngPGv12lv1ESn1ORD
fKaNRbV3WZAU3ve4wATT7KuUlCY0y+QYO722HA7yPGAn
-----END RSA PRIVATE KEY-----
";

const ACCOUNT: u64 = 76_561_198_000_000_001;
const OTHER_ACCOUNT: u64 = 76_561_198_000_000_002;
const APP: u32 = 440;
const NOW: u32 = 1_800_000_000;

fn private_key() -> RsaPrivateKey {
    RsaPrivateKey::from_pkcs1_pem(TEST_KEY).unwrap()
}

/// Minimal ticket body, then signature.
fn ticket(steam_id: u64, app_id: u32, expires: u32) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&4u32.to_le_bytes());
    body.extend_from_slice(&steam_id.to_le_bytes());
    body.extend_from_slice(&app_id.to_le_bytes());
    body.extend_from_slice(&[0; 8]);
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&(expires - 86_400).to_le_bytes());
    body.extend_from_slice(&expires.to_le_bytes());
    body.extend_from_slice(&1u16.to_le_bytes());
    body.extend_from_slice(&123u32.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    let len = u32::try_from(body.len()).unwrap();
    body[..4].copy_from_slice(&len.to_le_bytes());
    let digest = Sha1::digest(&body);
    let signature = private_key()
        .sign(Pkcs1v15Sign::new::<Sha1>(), &digest)
        .unwrap();
    body.extend_from_slice(&signature);
    body
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "aurelia-ticket-cache-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn cache(dir: &TempDir) -> TicketCache {
    TicketCache::new(dir.0.clone(), private_key().to_public_key())
}

#[test]
fn steam_system_key_is_1024_bit() {
    use rsa::traits::PublicKeyParts;
    let key = steam_system_key();
    assert_eq!(key.size(), 128);
    assert_eq!(key.e(), &BigUint::from(17u32));
}

#[test]
fn parse_reads_signed_fields() {
    let bytes = ticket(ACCOUNT, APP, NOW + 100);
    let parsed = parse_signed(&bytes, &private_key().to_public_key()).unwrap();
    assert_eq!(
        parsed,
        OwnershipTicket {
            steam_id: ACCOUNT,
            app_id: APP,
            issued: NOW + 100 - 86_400,
            expires: NOW + 100,
        }
    );
}

#[test]
fn parse_rejects_bad_signature_and_steam_key_mismatch() {
    let mut bytes = ticket(ACCOUNT, APP, NOW + 100);
    assert!(parse_signed(&bytes, &steam_system_key()).is_none());
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    assert!(parse_signed(&bytes, &private_key().to_public_key()).is_none());
    let unsigned = &ticket(ACCOUNT, APP, NOW + 100)[..bytes.len() - SIGNATURE_LEN];
    assert!(parse_signed(unsigned, &private_key().to_public_key()).is_none());
    assert!(parse_signed(&[], &private_key().to_public_key()).is_none());
}

#[test]
fn empty_cache_fails_closed_offline() {
    let dir = TempDir::new("empty");
    let cache = cache(&dir);
    assert!(cache.load(ACCOUNT, APP, NOW).is_none());
    let source = TicketSource::Offline {
        steam_id: Some(ACCOUNT),
    };
    assert!(cache.resolve(source, APP, NOW).is_err());
    let unknown = TicketSource::Offline { steam_id: None };
    assert!(cache.resolve(unknown, APP, NOW).is_err());
}

#[test]
fn online_ticket_is_cached_and_served_offline() {
    let dir = TempDir::new("roundtrip");
    let cache = cache(&dir);
    let bytes = ticket(ACCOUNT, APP, NOW + 3600);
    let online = TicketSource::Online {
        steam_id: ACCOUNT,
        fetched: Ok(bytes.clone()),
    };
    assert_eq!(cache.resolve(online, APP, NOW).unwrap(), bytes);
    let offline = TicketSource::Offline {
        steam_id: Some(ACCOUNT),
    };
    assert_eq!(cache.resolve(offline, APP, NOW).unwrap(), bytes);
}

#[test]
fn online_failure_never_falls_back_to_cache() {
    let dir = TempDir::new("online-fail");
    let cache = cache(&dir);
    cache
        .store(ACCOUNT, APP, &ticket(ACCOUNT, APP, NOW + 3600))
        .unwrap();
    let online = TicketSource::Online {
        steam_id: ACCOUNT,
        fetched: Err(anyhow!("Steam said no")),
    };
    assert!(cache.resolve(online, APP, NOW).is_err());
}

#[test]
fn expired_ticket_is_refused_and_removed() {
    let dir = TempDir::new("expired");
    let cache = cache(&dir);
    cache
        .store(ACCOUNT, APP, &ticket(ACCOUNT, APP, NOW))
        .unwrap();
    assert!(cache.load(ACCOUNT, APP, NOW - 1).is_some());
    assert!(cache.load(ACCOUNT, APP, NOW).is_none());
    assert!(!cache.path(ACCOUNT, APP).exists());
    let offline = TicketSource::Offline {
        steam_id: Some(ACCOUNT),
    };
    assert!(cache.resolve(offline, APP, NOW).is_err());
}

#[test]
fn account_mismatch_is_refused() {
    let dir = TempDir::new("account");
    let cache = cache(&dir);
    let foreign = ticket(OTHER_ACCOUNT, APP, NOW + 3600);
    assert!(cache.store(ACCOUNT, APP, &foreign).is_err());
    assert!(
        cache
            .store(ACCOUNT, APP + 1, &ticket(ACCOUNT, APP, NOW + 3600))
            .is_err()
    );

    std::fs::create_dir_all(&dir.0).unwrap();
    std::fs::write(cache.path(ACCOUNT, APP), &foreign).unwrap();
    assert!(cache.load(ACCOUNT, APP, NOW).is_none());

    cache.store(OTHER_ACCOUNT, APP, &foreign).unwrap();
    let offline = TicketSource::Offline {
        steam_id: Some(ACCOUNT),
    };
    assert!(cache.resolve(offline, APP, NOW).is_err());
}

#[test]
fn unsigned_ticket_is_not_cached() {
    let dir = TempDir::new("unsigned");
    let cache = cache(&dir);
    let mut bytes = ticket(ACCOUNT, APP, NOW + 3600);
    bytes.truncate(bytes.len() - SIGNATURE_LEN);
    assert!(cache.store(ACCOUNT, APP, &bytes).is_err());
    let online = TicketSource::Online {
        steam_id: ACCOUNT,
        fetched: Ok(bytes.clone()),
    };
    assert_eq!(cache.resolve(online, APP, NOW).unwrap(), bytes);
    assert!(cache.load(ACCOUNT, APP, NOW).is_none());
}
