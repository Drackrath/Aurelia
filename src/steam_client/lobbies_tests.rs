use super::*;
use steam_vent_proto::steammessages_clientserver_mms::cmsg_client_mmsjoin_lobby_response::Member;

fn pairs(list: &[(&str, &str)]) -> Metadata {
    list.iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[test]
fn metadata_binary_layout() {
    let blob = encode_metadata(&pairs(&[("mode", "ctf")]));
    assert_eq!(
        blob,
        [
            0, 0, 1, b'm', b'o', b'd', b'e', 0, b'c', b't', b'f', 0, 8, 8
        ]
    );
    assert_eq!(encode_metadata(&[]), [0, 0, 8, 8]);
}

#[test]
fn metadata_round_trip_and_junk() {
    let meta = pairs(&[("a", "1"), ("name", "Lobby ü")]);
    assert_eq!(decode_metadata(&encode_metadata(&meta)), meta);
    assert!(decode_metadata(&[]).is_empty());
    assert!(decode_metadata(&[1, 2, 3]).is_empty());
    // Truncated pair stops cleanly.
    assert_eq!(decode_metadata(&[0, 0, 1, b'k', 0, b'v']), Vec::new());
}

#[test]
fn join_reply_maps_members() {
    let mut res = CMsgClientMMSJoinLobbyResponse::new();
    res.set_steam_id_lobby(109_775_240_000_000_001);
    res.set_steam_id_owner(7);
    res.set_chat_room_enter_response(1);
    res.set_max_members(4);
    res.set_metadata(encode_metadata(&pairs(&[("map", "dm1")])));
    let mut m = Member::new();
    m.set_steam_id(7);
    m.set_persona_name("Owner".into());
    m.set_metadata(encode_metadata(&pairs(&[("ready", "1")])));
    res.members.push(m);
    let joined = joined_lobby(&res);
    assert_eq!(joined.enter_response, 1);
    assert_eq!(joined.state.owner, 7);
    assert_eq!(joined.state.num_members, 1);
    assert_eq!(joined.state.metadata, pairs(&[("map", "dm1")]));
    assert_eq!(joined.state.members[0].metadata, pairs(&[("ready", "1")]));
}

#[test]
fn not_logged_in_fails() {
    let client = SteamClient::new().unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    assert!(rt.block_on(client.create_lobby(480, 2, 4)).is_err());
    assert!(client.lobby_events(480).is_err());
}
