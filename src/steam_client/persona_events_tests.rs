use super::*;
use steam_vent_proto::steammessages_clientserver_friends::cmsg_client_persona_state::friend::KV;

fn friend(id: u64) -> PersonaFriend {
    let mut fr = PersonaFriend::new();
    fr.set_friendid(id);
    fr
}

fn message(friends: Vec<PersonaFriend>) -> CMsgClientPersonaState {
    let mut msg = CMsgClientPersonaState::new();
    msg.friends = friends;
    msg
}

#[test]
fn first_sight_reports_known_fields() {
    let mut fr = friend(7);
    fr.set_player_name("Pal".into());
    fr.set_persona_state(1);
    let mut known = HashMap::new();
    let events = persona_events(&mut known, &message(vec![fr]));
    assert_eq!(
        events,
        vec![PersonaEvent {
            steam_id: 7,
            flags: change::NAME | change::STATUS | change::COME_ONLINE,
            app_id: 0,
        }]
    );
}

#[test]
fn only_changes_are_reported() {
    let mut known = HashMap::new();
    let mut fr = friend(7);
    fr.set_player_name("Pal".into());
    fr.set_persona_state(1);
    persona_events(&mut known, &message(vec![fr.clone()]));
    // Same again: nothing.
    assert!(persona_events(&mut known, &message(vec![fr])).is_empty());

    let mut game = friend(7);
    game.set_game_played_app_id(480);
    let events = persona_events(&mut known, &message(vec![game]));
    assert_eq!(events[0].flags, change::GAME_PLAYED);
    assert_eq!(events[0].app_id, 480);

    let mut off = friend(7);
    off.set_persona_state(0);
    let flags = persona_events(&mut known, &message(vec![off]))[0].flags;
    assert_eq!(flags, change::STATUS | change::GONE_OFFLINE);

    let mut avatar = friend(7);
    avatar.set_avatar_hash(vec![0xAB; 20]);
    let flags = persona_events(&mut known, &message(vec![avatar]))[0].flags;
    assert_eq!(flags, change::AVATAR);
}

#[test]
fn rich_presence_is_flagged() {
    let mut known = HashMap::new();
    let mut fr = friend(9);
    let mut kv = KV::new();
    kv.set_key("status".into());
    kv.set_value("In lobby".into());
    fr.rich_presence.push(kv);
    let events = persona_events(&mut known, &message(vec![fr]));
    assert_eq!(events[0].flags, change::RICH_PRESENCE);
}

#[test]
fn not_logged_in_fails() {
    assert!(SteamClient::new().unwrap().persona_events().is_err());
}
