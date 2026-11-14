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
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(1, RELATIONSHIP_FRIEND)]),
    );
    // A second full snapshot without entry 1 should drop it.
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(2, RELATIONSHIP_FRIEND)]),
    );
    assert!(!roster.contains_key(&1));
    assert!(roster.contains_key(&2));
}

#[test]
fn persona_state_fills_fields() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
    apply_persona_state(&mut roster, &persona_state(&[(10, "Alice", 1, 440)]));

    let f = &roster[&10];
    assert_eq!(f.persona_name.as_deref(), Some("Alice"));
    assert_eq!(f.persona_state, Some(1));
    assert_eq!(f.game_app_id, Some(440));
}

#[test]
fn persona_state_does_not_clobber_known_name_with_empty() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
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
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
    assert!(roster.contains_key(&10));

    // Incremental update marking 10 as None (0) removes it without clearing.
    apply_friends_list(&mut roster, &friends_list(true, &[(10, 0)]));
    assert!(!roster.contains_key(&10));
}

#[test]
fn incremental_upsert_preserves_persona_fields() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
    apply_persona_state(&mut roster, &persona_state(&[(10, "Alice", 1, 440)]));

    // An incremental relationship change must not wipe the known name/state.
    apply_friends_list(&mut roster, &friends_list(true, &[(10, 6)]));
    assert_eq!(roster[&10].relationship, 6);
    assert_eq!(roster[&10].persona_name.as_deref(), Some("Alice"));
    assert_eq!(roster[&10].persona_state, Some(1));
}

/// A reply carrying only the name.
fn name_only(id: u64, name: &str) -> CMsgClientPersonaState {
    let mut msg = CMsgClientPersonaState::new();
    let mut f = cmsg_client_persona_state::Friend::new();
    f.set_friendid(id);
    f.set_player_name(name.to_string());
    msg.friends.push(f);
    msg
}

#[test]
fn reply_without_status_keeps_known_state_and_game() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
    let mut full = persona_state(&[(10, "Alice", 3, 440)]);
    full.friends[0].set_game_name("Team Fortress 2".to_string());
    apply_persona_state(&mut roster, &full);

    apply_persona_state(&mut roster, &name_only(10, "Alice2"));

    let f = &roster[&10];
    assert_eq!(f.persona_name.as_deref(), Some("Alice2"));
    assert_eq!(f.persona_state, Some(3));
    assert_eq!(f.game_app_id, Some(440));
    assert_eq!(f.game_name.as_deref(), Some("Team Fortress 2"));
}

#[test]
fn reply_without_status_leaves_unknown_state_unset() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
    apply_persona_state(&mut roster, &name_only(10, "Alice"));
    assert_eq!(roster[&10].persona_state, None);
}

#[test]
fn stopping_a_game_clears_its_name() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
    let mut playing = persona_state(&[(10, "Alice", 1, 440)]);
    playing.friends[0].set_game_name("Team Fortress 2".to_string());
    apply_persona_state(&mut roster, &playing);
    apply_persona_state(&mut roster, &persona_state(&[(10, "Alice", 1, 0)]));

    assert_eq!(roster[&10].game_app_id, None);
    assert_eq!(roster[&10].game_name, None);
}

#[test]
fn full_snapshot_keeps_persona_of_remaining_friends() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(
            false,
            &[(10, RELATIONSHIP_FRIEND), (20, RELATIONSHIP_FRIEND)],
        ),
    );
    apply_persona_state(&mut roster, &persona_state(&[(10, "Alice", 3, 440)]));

    apply_friends_list(
        &mut roster,
        &friends_list(false, &[(10, RELATIONSHIP_FRIEND)]),
    );
    assert!(!roster.contains_key(&20));
    assert_eq!(roster[&10].persona_name.as_deref(), Some("Alice"));
    assert_eq!(roster[&10].persona_state, Some(3));
}

#[test]
fn unnamed_friend_ids_skips_named_and_non_friends() {
    let mut roster = Roster::new();
    apply_friends_list(
        &mut roster,
        &friends_list(
            false,
            &[
                (10, RELATIONSHIP_FRIEND),
                (20, RELATIONSHIP_FRIEND),
                (30, 2),
            ],
        ),
    );
    apply_persona_state(&mut roster, &persona_state(&[(10, "Alice", 1, 0)]));
    assert_eq!(unnamed_friend_ids(&roster), vec![20]);
}

#[test]
fn persona_state_tracks_lobby() {
    let mut roster = Roster::new();
    let mut msg = persona_state(&[(10, "Alice", 1, 480)]);
    msg.friends[0].set_game_lobby_id(109_775_241_000_000_001);
    apply_persona_state(&mut roster, &msg);
    assert_eq!(roster[&10].game_lobby_id, Some(109_775_241_000_000_001));
    let mut zero = persona_state(&[(10, "Alice", 1, 480)]);
    zero.friends[0].set_game_lobby_id(0);
    apply_persona_state(&mut roster, &zero);
    assert_eq!(roster[&10].game_lobby_id, None, "left the lobby");
    apply_persona_state(&mut roster, &msg);
    apply_persona_state(&mut roster, &persona_state(&[(10, "Alice", 1, 0)]));
    assert_eq!(roster[&10].game_lobby_id, None, "left the game");
}
