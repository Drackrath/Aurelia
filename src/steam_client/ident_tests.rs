use super::*;
use crate::core::error::ErrorKind;
use crate::steam_client::Friend;

fn friend(id: u64, name: &str) -> Friend {
    Friend {
        steam_id: id,
        relationship: 3,
        persona_name: Some(name.to_string()),
        persona_state: None,
        game_app_id: None,
        game_name: None,
    }
}

#[test]
fn ids_and_profile_urls_parse_locally() {
    assert_eq!(parse_ident("me").unwrap(), Ident::Me);
    assert_eq!(parse_ident("ME").unwrap(), Ident::Me);
    assert_eq!(
        parse_ident("76561197960287930").unwrap(),
        Ident::SteamId(76561197960287930)
    );
    assert_eq!(
        parse_ident("https://steamcommunity.com/profiles/76561197960287930/").unwrap(),
        Ident::SteamId(76561197960287930)
    );
}

#[test]
fn vanity_urls_are_rejected() {
    let err = parse_ident("https://steamcommunity.com/id/gabelogannewell").unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidInput);
    assert!(err.message.contains("gabelogannewell"));
}

#[test]
fn small_numbers_are_not_steam_ids() {
    assert_eq!(parse_ident("12345").unwrap_err().kind, ErrorKind::InvalidInput);
    assert_eq!(parse_ident("   ").unwrap_err().kind, ErrorKind::InvalidInput);
}

#[test]
fn other_text_is_a_name() {
    assert_eq!(parse_ident("Rabscuttle").unwrap(), Ident::Name("Rabscuttle".into()));
}

#[test]
fn name_matching_prefers_exact_then_prefix() {
    let friends = vec![friend(1, "Alex"), friend(2, "Alexander"), friend(3, "Bob")];
    assert_eq!(match_friend_name("alex", &friends).unwrap().steam_id, 1);
    assert_eq!(match_friend_name("bo", &friends).unwrap().steam_id, 3);
    assert_eq!(
        match_friend_name("al", &[friend(2, "Alexander"), friend(4, "Alma")]).unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(match_friend_name("zed", &friends).unwrap_err().kind, ErrorKind::NotFound);
}
