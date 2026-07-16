use super::*;
use steam_vent_proto::steammessages_clientserver_friends::{
    cmsg_client_friends_list, cmsg_client_persona_state,
};

fn friends_list(incremental: bool, entries: &[(u64, u32)]) -> CMsgClientFriendsList {
    let mut msg = CMsgClientFriendsList::new();
    msg.set_bincremental(incremental);
    for &(id, rel) in entries {
        let mut f = cmsg_client_friends_list::Friend::new();
        f.set_ulfriendid(id);
        f.set_efriendrelationship(rel);
        msg.friends.push(f);
    }
    msg
}

fn persona_state(entries: &[(u64, &str, u32, u32)]) -> CMsgClientPersonaState {
    let mut msg = CMsgClientPersonaState::new();
    for &(id, name, state, app) in entries {
        let mut f = cmsg_client_persona_state::Friend::new();
        f.set_friendid(id);
        f.set_player_name(name.to_string());
        f.set_persona_state(state);
        f.set_game_played_app_id(app);
        msg.friends.push(f);
    }
    msg
}

#[test]
fn full_snapshot_populates_relationships() {
    let mut roster = Roster::new();
    let msg = friends_list(false, &[(10, RELATIONSHIP_FRIEND), (20, 2)]);
    apply_friends_list(&mut roster, &msg);

    assert_eq!(roster.len(), 2);
    assert_eq!(roster[&10].relationship, RELATIONSHIP_FRIEND);
    assert_eq!(roster[&20].relationship, 2);
    assert!(roster[&10].persona_name.is_none());
}

#[test]
fn full_snapshot_clears_previous_entries() {
    let mut roster = Roster::new();
    apply_friends_list(&mut roster, &friends_list(false, &[(1, RELATIONSHIP_FRIEND)]));
    // A second full snapshot without entry 1 should drop it.
    apply_friends_list(&mut roster, &friends_list(false, &[(2, RELATIONSHIP_FRIEND)]));
    assert!(!roster.contains_key(&1));
    assert!(roster.contains_key(&2));
}

#[test]
fn persona_state_fills_fields() {
    let mut roster = Roster::new();
    apply_friends_list(&mut roster, &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]));
    apply_persona_state(&mut roster, &persona_state(&[(10, "Alice", 1, 440)]));

    let f = &roster[&10];
    assert_eq!(f.persona_name.as_deref(), Some("Alice"));
    assert_eq!(f.persona_state, Some(1));
    assert_eq!(f.game_app_id, Some(440));
}

#[test]
fn persona_state_does_not_clobber_known_name_with_empty() {
    let mut roster = Roster::new();
    apply_friends_list(&mut roster, &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]));
    apply_persona_state(&mut roster, &persona_state(&[(10, "Alice", 1, 0)]));
    // A later update with an empty name must not overwrite "Alice".
    apply_persona_state(&mut roster, &persona_state(&[(10, "", 0, 0)]));

    assert_eq!(roster[&10].persona_name.as_deref(), Some("Alice"));
    // Persona state and game still update from the empty-name message.
    assert_eq!(roster[&10].persona_state, Some(0));
    assert_eq!(roster[&10].game_app_id, None);
}

#[test]
fn incremental_relationship_zero_removes_entry() {
    let mut roster = Roster::new();
    apply_friends_list(&mut roster, &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]));
    assert!(roster.contains_key(&10));

    // Incremental update marking 10 as None (0) removes it without clearing.
    apply_friends_list(&mut roster, &friends_list(true, &[(10, 0)]));
    assert!(!roster.contains_key(&10));
}

#[test]
fn incremental_upsert_preserves_persona_fields() {
    let mut roster = Roster::new();
    apply_friends_list(&mut roster, &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]));
    apply_persona_state(&mut roster, &persona_state(&[(10, "Alice", 1, 440)]));

    // An incremental relationship change must not wipe the known name/state.
    apply_friends_list(&mut roster, &friends_list(true, &[(10, 6)]));
    assert_eq!(roster[&10].relationship, 6);
    assert_eq!(roster[&10].persona_name.as_deref(), Some("Alice"));
    assert_eq!(roster[&10].persona_state, Some(1));
}
