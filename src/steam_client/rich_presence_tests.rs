use super::*;

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn encode_matches_binary_keyvalues() {
    let bytes = encode_kv(&pairs(&[("status", "In menu")]));
    let mut expected = vec![0];
    expected.extend_from_slice(b"RP\0");
    expected.push(1);
    expected.extend_from_slice(b"status\0In menu\0");
    expected.extend_from_slice(&[8, 8]);
    assert_eq!(bytes, expected);
    assert_eq!(encode_kv(&[]), b"\0RP\0\x08\x08");
}

#[test]
fn decode_round_trips_and_flattens() {
    let list = pairs(&[
        ("status", "Lobby"),
        ("steam_display", "#Menu"),
        ("connect", "+connect 1.2.3.4"),
    ]);
    assert_eq!(decode_kv(&encode_kv(&list)), Some(list));

    let mut nested = vec![0];
    nested.extend_from_slice(b"RP\0");
    nested.push(0);
    nested.extend_from_slice(b"inner\0");
    nested.push(2);
    nested.extend_from_slice(b"score\0");
    nested.extend_from_slice(&(-5i32).to_le_bytes());
    nested.push(8);
    nested.push(7);
    nested.extend_from_slice(b"big\0");
    nested.extend_from_slice(&7u64.to_le_bytes());
    nested.extend_from_slice(&[8, 8]);
    assert_eq!(
        decode_kv(&nested),
        Some(pairs(&[("score", "-5"), ("big", "7")]))
    );
}

#[test]
fn decode_rejects_garbage() {
    assert_eq!(decode_kv(&[]), Some(Vec::new()));
    assert_eq!(decode_kv(&[1, b'x']), None);
    let full = encode_kv(&pairs(&[("a", "b")]));
    assert_eq!(decode_kv(&full[..full.len() - 2]), None);
    assert_eq!(decode_kv(b"\0RP\0\x09x\0\x08"), None);
}
