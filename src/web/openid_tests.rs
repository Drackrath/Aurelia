//! Tests for the OpenID 2.0 assertion validation (the pure, non-network parts).

use super::*;

const RETURN_TO: &str = "http://127.0.0.1:43210/callback?aurelia_nonce=00000000000000000000000000abcdef";
const STEAM_ID: &str = "76561197990935091";

/// A well-formed Steam `id_res` assertion, as `(key, value)` pairs.
fn good_params() -> Vec<(String, String)> {
    let claimed = format!("https://steamcommunity.com/openid/id/{STEAM_ID}");
    [
        ("openid.ns", "http://specs.openid.net/auth/2.0"),
        ("openid.mode", "id_res"),
        ("openid.op_endpoint", STEAM_OPENID_ENDPOINT),
        ("openid.claimed_id", claimed.as_str()),
        ("openid.identity", claimed.as_str()),
        ("openid.return_to", RETURN_TO),
        ("openid.response_nonce", "2026-07-16T12:00:00Zabcdef"),
        ("openid.assoc_handle", "1234567890"),
        (
            "openid.signed",
            "signed,op_endpoint,claimed_id,identity,return_to,response_nonce,assoc_handle",
        ),
        ("openid.sig", "dGVzdA=="),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

fn with(params: Vec<(String, String)>, key: &str, value: &str) -> Vec<(String, String)> {
    params
        .into_iter()
        .map(|(k, v)| {
            if k == key {
                (k, value.to_string())
            } else {
                (k, v)
            }
        })
        .collect()
}

#[test]
fn accepts_a_well_formed_assertion() {
    let id = validate_assertion(&good_params(), RETURN_TO).unwrap();
    assert_eq!(id, STEAM_ID.parse::<u64>().unwrap());
}

#[test]
fn rejects_a_cancelled_sign_in() {
    let err = validate_assertion(&with(good_params(), "openid.mode", "cancel"), RETURN_TO)
        .unwrap_err();
    assert!(err.to_string().contains("cancelled"));
}

#[test]
fn rejects_a_foreign_op_endpoint() {
    let params = with(good_params(), "openid.op_endpoint", "https://evil.example/openid");
    assert!(validate_assertion(&params, RETURN_TO).is_err());
}

#[test]
fn rejects_a_return_to_mismatch() {
    // Same shape, different nonce — an assertion minted for another attempt.
    let other = RETURN_TO.replace("abcdef", "fedcba");
    let params = with(good_params(), "openid.return_to", &other);
    assert!(validate_assertion(&params, RETURN_TO).is_err());
}

#[test]
fn rejects_when_a_required_field_is_unsigned() {
    // `return_to` missing from the signed list — could have been tampered with.
    let params = with(
        good_params(),
        "openid.signed",
        "signed,op_endpoint,claimed_id,identity,response_nonce,assoc_handle",
    );
    let err = validate_assertion(&params, RETURN_TO).unwrap_err();
    assert!(err.to_string().contains("return_to"));
}

#[test]
fn rejects_identity_claimed_id_disagreement() {
    let params = with(
        good_params(),
        "openid.identity",
        "https://steamcommunity.com/openid/id/76561197990935092",
    );
    assert!(validate_assertion(&params, RETURN_TO).is_err());
}

#[test]
fn steam_id_parses_from_both_url_schemes() {
    for scheme in ["https", "http"] {
        let claimed = format!("{scheme}://steamcommunity.com/openid/id/{STEAM_ID}");
        assert_eq!(
            steam_id_from_claimed_id(&claimed).unwrap(),
            STEAM_ID.parse::<u64>().unwrap()
        );
    }
}

#[test]
fn steam_id_rejects_malformed_claims() {
    for claimed in [
        "https://steamcommunity.com/openid/id/1234",              // too short
        "https://steamcommunity.com/openid/id/7656119799093509x", // non-digit
        "https://steamcommunity.com/openid/id/10000000000000000", // below individual range
        "https://example.com/openid/id/76561197990935091",        // wrong host
    ] {
        assert!(steam_id_from_claimed_id(claimed).is_err(), "{claimed}");
    }
}

#[test]
fn query_pairs_decode_percent_encoding() {
    let pairs = parse_query_pairs(
        "/callback?openid.return_to=http%3A%2F%2F127.0.0.1%3A43210%2Fcallback&openid.mode=id_res",
    )
    .unwrap();
    assert_eq!(
        pairs[0],
        (
            "openid.return_to".to_string(),
            "http://127.0.0.1:43210/callback".to_string()
        )
    );
    assert_eq!(pairs[1].1, "id_res");
}

#[test]
fn verification_response_parsing() {
    assert!(verification_says_valid(
        "ns:http://specs.openid.net/auth/2.0\nis_valid:true\n"
    ));
    assert!(!verification_says_valid(
        "ns:http://specs.openid.net/auth/2.0\nis_valid:false\n"
    ));
    // Only the exact key:value line counts.
    assert!(!verification_says_valid("note:is_valid:true is expected"));
}

#[test]
fn request_target_requires_get() {
    assert_eq!(
        request_target("GET /callback?a=b HTTP/1.1\r\nHost: x\r\n\r\n"),
        Some("/callback?a=b")
    );
    assert_eq!(request_target("POST /callback HTTP/1.1\r\n\r\n"), None);
    assert_eq!(request_target(""), None);
}
