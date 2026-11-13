use super::*;

#[test]
fn parses_set_cookie_name_and_value() {
    assert_eq!(
        parse_set_cookie("webTradeEligibility=%7B%22allowed%22%3A1%7D; path=/; secure"),
        Some((
            "webTradeEligibility".to_string(),
            "%7B%22allowed%22%3A1%7D".to_string()
        ))
    );
    assert_eq!(parse_set_cookie("noequals; path=/"), None);
    assert_eq!(parse_set_cookie("=x"), None);
}

#[test]
fn merge_adds_new_and_keeps_auth_cookies() {
    let existing = "steamLoginSecure=1%7C%7Ctok; sessionid=abc; steamCountry=DE";
    let new = vec![
        ("webTradeEligibility".to_string(), "e1".to_string()),
        ("sessionid".to_string(), "evil".to_string()),
        ("steamCountry".to_string(), "US".to_string()),
    ];
    let merged = merge_cookies(existing, &new);
    assert_eq!(
        merged,
        "steamLoginSecure=1%7C%7Ctok; sessionid=abc; steamCountry=US; webTradeEligibility=e1"
    );

    // Merging again doesn't duplicate.
    let again = merge_cookies(&merged, &[("webTradeEligibility".into(), "e2".into())]);
    assert_eq!(again.matches("webTradeEligibility").count(), 1);
    assert!(again.ends_with("webTradeEligibility=e2"));
}

#[test]
fn ineligible_error_is_typed_access_denied() {
    let err = ineligible_error();
    let typed = err.downcast_ref::<TypedError>().unwrap();
    assert_eq!(typed.kind, ErrorKind::AccessDenied);
}
