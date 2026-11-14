use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

const SID: u64 = 76_561_197_960_287_930;
const APP: u32 = 480;
const NOW: u32 = 1_800_000_000;

/// RFC 8032 test 1 seed.
const SEED: [u8; 32] = [
    0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4,
    0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
];
const PUBLIC: [u8; 32] = [
    0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07, 0x3a,
    0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07, 0x51, 0x1a,
];

fn reply_for(public: &[u8], expiry: u32) -> CMsgClientNetworkingCertReply {
    let mut cert = CMsgSteamDatagramCertificate::new();
    cert.set_key_type(EKeyType::ED25519);
    cert.set_key_data(public.to_vec());
    cert.set_identity_string(format!("steamid:{SID}"));
    cert.set_time_created(NOW - 10);
    cert.set_time_expiry(expiry);
    cert.app_ids.push(APP);
    let mut reply = CMsgClientNetworkingCertReply::new();
    reply.set_cert(cert.write_to_bytes().unwrap());
    reply.set_ca_key_id(0x1234_5678_9abc_def0);
    reply.set_ca_signature(vec![7; 64]);
    reply
}

/// Answers by echoing the raw key.
fn signer(
    expiry: u32,
) -> impl FnOnce(Vec<u8>) -> std::future::Ready<Result<CMsgClientNetworkingCertReply>> {
    move |key_data| std::future::ready(Ok(reply_for(&key_data, expiry)))
}

#[test]
fn seed_gives_the_rfc_public_key() {
    let key = NetworkingKey::from_seed(Zeroizing::new(SEED)).unwrap();
    assert_eq!(key.public(), &PUBLIC);
    assert_eq!(format!("{key:?}"), "NetworkingKey(..)");
}

#[test]
fn generated_keys_differ() {
    let a = NetworkingKey::generate().unwrap();
    let b = NetworkingKey::generate().unwrap();
    assert_ne!(a.public(), b.public());
}

#[test]
fn raw_key_data_is_the_public_key() {
    let key = NetworkingKey::from_seed(Zeroizing::new(SEED)).unwrap();
    assert_eq!(key.key_data(KeyFormat::Raw, SID), PUBLIC.to_vec());
}

#[test]
fn request_key_data_is_a_gns_request() {
    let key = NetworkingKey::from_seed(Zeroizing::new(SEED)).unwrap();
    let bytes = key.key_data(KeyFormat::Request, SID);
    let request = CMsgSteamDatagramCertificateRequest::parse_from_bytes(&bytes).unwrap();
    assert_eq!(request.cert.key_type(), EKeyType::ED25519);
    assert_eq!(request.cert.key_data(), PUBLIC);
    assert_eq!(request.cert.identity_string(), format!("steamid:{SID}"));
}

#[test]
fn key_format_parses() {
    assert_eq!(KeyFormat::parse(""), Some(KeyFormat::Raw));
    assert_eq!(KeyFormat::parse("RAW"), Some(KeyFormat::Raw));
    assert_eq!(KeyFormat::parse(" request "), Some(KeyFormat::Request));
    assert_eq!(KeyFormat::parse("pem"), None);
    assert_eq!(KeyFormat::default(), KeyFormat::Raw);
}

#[test]
fn cert_request_carries_app_and_key() {
    let request = cert_request(APP, PUBLIC.to_vec());
    assert_eq!(request.app_id(), APP);
    assert_eq!(request.key_data(), PUBLIC);
    assert_eq!(
        <CertRequest as RpcMessageWithKind>::KIND,
        EMsg::k_EMsgClientNetworkingCertRequest
    );
    assert_eq!(
        <CertReply as RpcMessageWithKind>::KIND,
        EMsg::k_EMsgClientNetworkingCertRequestResponse
    );
}

#[test]
fn reply_for_another_key_is_refused() {
    let key = NetworkingKey::from_seed(Zeroizing::new(SEED)).unwrap();
    let err = NetworkingCert::from_reply(&reply_for(&[1; 32], NOW + 100), &key, NOW).unwrap_err();
    assert!(err.to_string().contains("another key"), "{err}");
}

#[test]
fn empty_or_expired_reply_is_refused() {
    let key = NetworkingKey::from_seed(Zeroizing::new(SEED)).unwrap();
    let empty = CMsgClientNetworkingCertReply::new();
    assert!(NetworkingCert::from_reply(&empty, &key, NOW).is_err());
    assert!(NetworkingCert::from_reply(&reply_for(&PUBLIC, NOW), &key, NOW).is_err());
}

#[test]
fn signed_blob_carries_cert_and_private_key() {
    let key = NetworkingKey::from_seed(Zeroizing::new(SEED)).unwrap();
    let reply = reply_for(&PUBLIC, NOW + 86_400);
    let cert = NetworkingCert::from_reply(&reply, &key, NOW).unwrap();
    assert_eq!(cert.expiry, NOW + 86_400);
    assert_eq!(cert.created, NOW - 10);
    assert_eq!(cert.app_ids, vec![APP]);
    let signed =
        CMsgSteamDatagramCertificateSigned::parse_from_bytes(&cert.signed_with_key(&key)).unwrap();
    assert_eq!(signed.cert(), reply.cert());
    assert_eq!(signed.ca_key_id(), reply.ca_key_id());
    assert_eq!(signed.ca_signature(), reply.ca_signature());
    assert_eq!(signed.private_key_data(), SEED);
}

#[tokio::test]
async fn fresh_cert_is_cached() {
    let mut cache = CertCache::default();
    let calls = AtomicU32::new(0);
    let fetch = |key: Vec<u8>| {
        calls.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Ok(reply_for(&key, NOW + 86_400)))
    };
    let first = cache
        .get(SID, APP, NOW, KeyFormat::Raw, fetch)
        .await
        .unwrap();
    let second = cache
        .get(SID, APP, NOW + 60, KeyFormat::Raw, fetch)
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(*first.signed, *second.signed);
    assert_eq!(second.expiry, NOW + 86_400);
}

/// Public key inside a grant.
fn grant_key(grant: &CertGrant) -> Vec<u8> {
    let signed = CMsgSteamDatagramCertificateSigned::parse_from_bytes(&grant.signed).unwrap();
    let cert = CMsgSteamDatagramCertificate::parse_from_bytes(signed.cert()).unwrap();
    cert.key_data().to_vec()
}

#[tokio::test]
async fn renewal_keeps_the_key() {
    let mut cache = CertCache::default();
    let keys = std::sync::Mutex::new(Vec::new());
    let expiry = NOW + RENEW_BEFORE + 100;
    let fetch = |key: Vec<u8>| {
        keys.lock().unwrap().push(key.clone());
        std::future::ready(Ok(reply_for(&key, expiry + 86_400)))
    };
    let first = cache
        .get(SID, APP, NOW, KeyFormat::Raw, signer(expiry))
        .await
        .unwrap();
    // Inside the renewal window now.
    let renewed = cache
        .get(SID, APP, NOW + 200, KeyFormat::Raw, fetch)
        .await
        .unwrap();
    assert_eq!(renewed.expiry, expiry + 86_400);
    assert_eq!(keys.lock().unwrap().as_slice(), &[grant_key(&first)]);
    assert_eq!(grant_key(&renewed), grant_key(&first));
    // Fresh again: no request.
    let again = cache
        .get(SID, APP, NOW + 300, KeyFormat::Raw, signer(0))
        .await
        .unwrap();
    assert_eq!(again.expiry, expiry + 86_400);
}

#[tokio::test]
async fn failed_renewal_serves_the_valid_cert() {
    let mut cache = CertCache::default();
    let expiry = NOW + RENEW_BEFORE;
    cache
        .get(SID, APP, NOW, KeyFormat::Raw, signer(expiry))
        .await
        .unwrap();
    let fail = |_| std::future::ready(Err(anyhow!("CM down")));
    let kept = cache
        .get(SID, APP, NOW + 1, KeyFormat::Raw, fail)
        .await
        .unwrap();
    assert_eq!(kept.expiry, expiry);
}

#[tokio::test]
async fn failures_are_not_retried_at_once() {
    let mut cache = CertCache::default();
    let calls = AtomicU32::new(0);
    let fail = |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Err(anyhow!("CM down")))
    };
    assert!(
        cache
            .get(SID, APP, NOW, KeyFormat::Raw, fail)
            .await
            .is_err()
    );
    let err = cache
        .get(SID, APP, NOW + RETRY_AFTER - 1, KeyFormat::Raw, fail)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("retry later"), "{err}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let later = NOW + RETRY_AFTER;
    let ok = cache
        .get(SID, APP, later, KeyFormat::Raw, signer(later + 86_400))
        .await;
    assert!(ok.is_ok());
}

#[tokio::test]
async fn keys_are_per_app_and_account() {
    let mut cache = CertCache::default();
    let mut publics = Vec::new();
    for (sid, app) in [(SID, APP), (SID, APP + 1), (SID + 1, APP)] {
        let grant = cache
            .get(sid, app, NOW, KeyFormat::Raw, signer(NOW + 86_400))
            .await
            .unwrap();
        let signed = CMsgSteamDatagramCertificateSigned::parse_from_bytes(&grant.signed).unwrap();
        publics.push(signed.private_key_data().to_vec());
    }
    assert_ne!(publics[0], publics[1]);
    assert_ne!(publics[0], publics[2]);
    cache.forget_account(SID);
    assert_eq!(cache.entries.len(), 1);
}

#[test]
fn grant_debug_hides_the_key() {
    let grant = CertGrant {
        expiry: 5,
        signed: Zeroizing::new(vec![0xAB; 8]),
    };
    let text = format!("{grant:?}");
    assert!(!text.contains("171") && !text.contains("AB"), "{text}");
}
