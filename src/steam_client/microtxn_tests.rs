use super::*;

fn kv_doc(name: &str, pairs: &[(&str, &str)]) -> Vec<u8> {
    let pairs: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    binary_kv::encode(name, &pairs)
}

fn body(order: &[(&str, &str)]) -> Vec<u8> {
    let mut out = vec![1];
    out.extend(kv_doc("MessageObject", order));
    out.extend(kv_doc("MessageObject", &[("balance", "1000")]));
    out
}

#[test]
fn parses_order_details() {
    let parsed = parse_auth_request(&body(&[
        ("appid", "480"),
        ("orderid", "123456789012"),
        ("transid", "987654321098"),
        ("currency", "EUR"),
    ]));
    let request = parsed.unwrap();
    assert_eq!(
        request,
        MicroTxnRequest {
            app_id: 480,
            order_id: 123_456_789_012,
            trans_id: 987_654_321_098,
        }
    );
    assert_eq!(
        request.approval_url(),
        "https://checkout.steampowered.com/checkout/approvetxn/987654321098/"
    );
}

#[test]
fn rejects_incomplete_requests() {
    assert_eq!(
        parse_auth_request(&body(&[("appid", "480"), ("orderid", "1")])),
        None
    );
    assert_eq!(
        parse_auth_request(&body(&[
            ("appid", "480"),
            ("orderid", "0"),
            ("transid", "5")
        ])),
        None
    );
    assert_eq!(parse_auth_request(&[]), None);
    assert_eq!(parse_auth_request(&[1, 9, 9]), None);
}

#[test]
fn pending_orders_are_taken_once() {
    let order = |id| MicroTxnRequest {
        app_id: 480,
        order_id: id,
        trans_id: id + 1,
    };
    record(order(9_000_001));
    record(order(9_000_002));
    record(order(9_000_001));
    let ids: Vec<u64> = pending()
        .iter()
        .map(|p| p.order_id)
        .filter(|id| *id >= 9_000_001 && *id <= 9_000_002)
        .collect();
    assert_eq!(ids, vec![9_000_002, 9_000_001]);
    assert_eq!(take(9_000_001), Some(order(9_000_001)));
    assert_eq!(take(9_000_001), None);
    assert!(take(9_000_002).is_some());
}

#[test]
fn second_document_does_not_confuse_the_first() {
    let mut raw = vec![1];
    // Node without top-level end.
    raw.extend_from_slice(
        b"\0MessageObject\0\x01appid\x00480\0\x01orderid\x005\0\x01transid\x006\0\x08",
    );
    raw.extend_from_slice(b"\0MessageObject\0\x01orderid\x009\0\x08");
    let request = parse_auth_request(&raw).unwrap();
    assert_eq!(
        (request.app_id, request.order_id, request.trans_id),
        (480, 5, 6)
    );
}
