use super::super::*;

#[test]
fn debug_never_shows_the_value() {
    let secret = Secret::new("eyJhbGciOi.refresh");
    assert!(!format!("{secret:?}").contains("eyJ"));
    assert!(!format!("{:?}", Some(&secret)).contains("refresh"));
}

#[test]
fn serde_round_trips_as_plain_string() {
    let json = serde_json::to_string(&Some(Secret::new("tok"))).unwrap();
    assert_eq!(json, "\"tok\"");
    let back: Option<Secret> = serde_json::from_str(&json).unwrap();
    assert_eq!(back.unwrap().expose(), "tok");
}
