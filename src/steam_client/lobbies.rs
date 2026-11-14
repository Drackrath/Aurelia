//! Steam lobbies over CM matchmaking messages.
use super::*;
use steam_vent_proto::steammessages_clientserver_mms::cmsg_client_mmsget_lobby_list::Filter;
use steam_vent_proto::steammessages_clientserver_mms::{
    CMsgClientMMSCreateLobby, CMsgClientMMSCreateLobbyResponse, CMsgClientMMSGetLobbyData,
    CMsgClientMMSGetLobbyList, CMsgClientMMSGetLobbyListResponse, CMsgClientMMSJoinLobby,
    CMsgClientMMSJoinLobbyResponse, CMsgClientMMSLeaveLobby, CMsgClientMMSLeaveLobbyResponse,
    CMsgClientMMSLobbyChatMsg, CMsgClientMMSLobbyData, CMsgClientMMSSendLobbyChatMsg,
    CMsgClientMMSSetLobbyData, CMsgClientMMSUserJoinedLobby, CMsgClientMMSUserLeftLobby,
};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

/// Key/value pairs, in order.
pub type Metadata = Vec<(String, String)>;

/// Binary KeyValues tags.
const KV_NONE: u8 = 0;
const KV_STRING: u8 = 1;
const KV_END: u8 = 8;

/// Metadata as binary KeyValues.
pub fn encode_metadata(pairs: &[(String, String)]) -> Vec<u8> {
    let mut out = vec![KV_NONE, 0];
    for (key, value) in pairs {
        out.push(KV_STRING);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        out.extend_from_slice(value.as_bytes());
        out.push(0);
    }
    out.extend_from_slice(&[KV_END, KV_END]);
    out
}

/// Flat string pairs of a blob.
pub fn decode_metadata(bytes: &[u8]) -> Metadata {
    fn cstr(bytes: &[u8], at: &mut usize) -> Option<String> {
        let len = bytes.get(*at..)?.iter().position(|b| *b == 0)?;
        let s = String::from_utf8_lossy(&bytes[*at..*at + len]).into_owned();
        *at += len + 1;
        Some(s)
    }
    let mut out = Vec::new();
    if bytes.first() != Some(&KV_NONE) {
        return out;
    }
    let mut at = 1;
    if cstr(bytes, &mut at).is_none() {
        return out;
    }
    while let Some(&tag) = bytes.get(at) {
        at += 1;
        match tag {
            KV_STRING => {
                let (Some(k), Some(v)) = (cstr(bytes, &mut at), cstr(bytes, &mut at)) else {
                    break;
                };
                out.push((k, v));
            }
            _ => break,
        }
    }
    out
}

/// One lobby member.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LobbyMember {
    pub steam_id: u64,
    pub name: String,
    pub metadata: Metadata,
}

/// Lobby state Steam reported.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LobbyState {
    pub lobby: u64,
    pub owner: u64,
    pub max_members: i32,
    pub lobby_type: i32,
    pub flags: i32,
    pub num_members: i32,
    pub metadata: Metadata,
    pub members: Vec<LobbyMember>,
}

/// Join answer plus state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JoinedLobby {
    pub enter_response: i32,
    pub state: LobbyState,
}

/// One list filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbyFilter {
    pub key: String,
    pub value: String,
    pub comparison: i32,
    pub filter_type: i32,
}

/// Complete data a game sets.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LobbyUpdate {
    pub lobby: u64,
    /// Zero for lobby data.
    pub member: u64,
    pub max_members: i32,
    pub lobby_type: i32,
    pub flags: i32,
    pub metadata: Metadata,
}

/// Something Steam pushed about lobbies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LobbyEvent {
    Data(LobbyState),
    Joined {
        lobby: u64,
        user: u64,
        name: String,
    },
    Left {
        lobby: u64,
        user: u64,
    },
    Chat {
        lobby: u64,
        sender: u64,
        message: Vec<u8>,
    },
}

fn members_of<'a>(list: impl Iterator<Item = (u64, &'a str, &'a [u8])>) -> Vec<LobbyMember> {
    list.map(|(steam_id, name, meta)| LobbyMember {
        steam_id,
        name: name.to_owned(),
        metadata: decode_metadata(meta),
    })
    .collect()
}

/// State from a lobby data push.
pub fn lobby_state(msg: &CMsgClientMMSLobbyData) -> LobbyState {
    LobbyState {
        lobby: msg.steam_id_lobby(),
        owner: msg.steam_id_owner(),
        max_members: msg.max_members(),
        lobby_type: msg.lobby_type(),
        flags: msg.lobby_flags(),
        num_members: msg.num_members(),
        metadata: decode_metadata(msg.metadata()),
        members: members_of(
            msg.members
                .iter()
                .map(|m| (m.steam_id(), m.persona_name(), m.metadata())),
        ),
    }
}

/// Join reply to our types.
pub fn joined_lobby(msg: &CMsgClientMMSJoinLobbyResponse) -> JoinedLobby {
    let members = members_of(
        msg.members
            .iter()
            .map(|m| (m.steam_id(), m.persona_name(), m.metadata())),
    );
    JoinedLobby {
        enter_response: msg.chat_room_enter_response(),
        state: LobbyState {
            lobby: msg.steam_id_lobby(),
            owner: msg.steam_id_owner(),
            max_members: msg.max_members(),
            lobby_type: msg.lobby_type(),
            flags: msg.lobby_flags(),
            num_members: i32::try_from(members.len()).unwrap_or(i32::MAX),
            metadata: decode_metadata(msg.metadata()),
            members,
        },
    }
}

/// Listed lobbies to our types.
pub fn listed_lobbies(msg: &CMsgClientMMSGetLobbyListResponse) -> Vec<LobbyState> {
    msg.lobbies
        .iter()
        .map(|l| LobbyState {
            lobby: l.steam_id(),
            max_members: l.max_members(),
            lobby_type: l.lobby_type(),
            flags: l.lobby_flags(),
            num_members: l.num_members(),
            metadata: decode_metadata(l.metadata()),
            ..LobbyState::default()
        })
        .collect()
}

fn mms_error(what: &str) -> impl FnOnce(steam_vent::NetworkError) -> anyhow::Error + '_ {
    move |e| anyhow!("Steam lobby {what} failed: {e}")
}

impl SteamClient {
    /// EResult and new lobby id.
    pub async fn create_lobby(&self, app_id: u32, lobby_type: i32, max: i32) -> Result<(i32, u64)> {
        let mut req = CMsgClientMMSCreateLobby::new();
        req.set_app_id(app_id);
        req.set_lobby_type(lobby_type);
        req.set_max_members(max);
        req.set_metadata(encode_metadata(&[]));
        let res: CMsgClientMMSCreateLobbyResponse = self
            .require_connection()?
            .job(req)
            .await
            .map_err(mms_error("create"))?;
        Ok((res.eresult(), res.steam_id_lobby()))
    }

    pub async fn join_lobby(&self, app_id: u32, lobby: u64) -> Result<JoinedLobby> {
        let mut req = CMsgClientMMSJoinLobby::new();
        req.set_app_id(app_id);
        req.set_steam_id_lobby(lobby);
        let res: CMsgClientMMSJoinLobbyResponse = self
            .require_connection()?
            .job(req)
            .await
            .map_err(mms_error("join"))?;
        Ok(joined_lobby(&res))
    }

    /// EResult of leaving.
    pub async fn leave_lobby(&self, app_id: u32, lobby: u64) -> Result<i32> {
        let mut req = CMsgClientMMSLeaveLobby::new();
        req.set_app_id(app_id);
        req.set_steam_id_lobby(lobby);
        let res: CMsgClientMMSLeaveLobbyResponse = self
            .require_connection()?
            .job(req)
            .await
            .map_err(mms_error("leave"))?;
        Ok(res.eresult())
    }

    /// EResult and matching lobbies.
    pub async fn lobby_list(
        &self,
        app_id: u32,
        count: i32,
        filters: &[LobbyFilter],
    ) -> Result<(i32, Vec<LobbyState>)> {
        let mut req = CMsgClientMMSGetLobbyList::new();
        req.set_app_id(app_id);
        req.set_num_lobbies_requested(count);
        for f in filters {
            let mut filter = Filter::new();
            filter.set_key(f.key.clone());
            filter.set_value(f.value.clone());
            filter.set_comparision(f.comparison);
            filter.set_filter_type(f.filter_type);
            req.filters.push(filter);
        }
        let res: CMsgClientMMSGetLobbyListResponse = self
            .require_connection()?
            .job(req)
            .await
            .map_err(mms_error("list"))?;
        Ok((res.eresult(), listed_lobbies(&res)))
    }

    /// Full lobby or member data.
    pub async fn set_lobby_data(&self, app_id: u32, update: &LobbyUpdate) -> Result<()> {
        let mut req = CMsgClientMMSSetLobbyData::new();
        req.set_app_id(app_id);
        req.set_steam_id_lobby(update.lobby);
        req.set_steam_id_member(update.member);
        req.set_max_members(update.max_members);
        req.set_lobby_type(update.lobby_type);
        req.set_lobby_flags(update.flags);
        req.set_metadata(encode_metadata(&update.metadata));
        self.require_connection()?
            .send(req)
            .await
            .map_err(mms_error("data update"))
    }

    pub async fn send_lobby_chat(&self, app_id: u32, lobby: u64, message: Vec<u8>) -> Result<()> {
        let mut req = CMsgClientMMSSendLobbyChatMsg::new();
        req.set_app_id(app_id);
        req.set_steam_id_lobby(lobby);
        req.set_lobby_message(message);
        self.require_connection()?
            .send(req)
            .await
            .map_err(mms_error("chat"))
    }

    /// Ask Steam to push lobby data.
    pub async fn request_lobby_data(&self, app_id: u32, lobby: u64) -> Result<()> {
        let mut req = CMsgClientMMSGetLobbyData::new();
        req.set_app_id(app_id);
        req.set_steam_id_lobby(lobby);
        self.require_connection()?
            .send(req)
            .await
            .map_err(mms_error("data request"))
    }

    /// Lobby pushes for one app.
    pub fn lobby_events(&self, app_id: u32) -> Result<mpsc::UnboundedReceiver<LobbyEvent>> {
        let connection = self.require_connection_owned()?;
        let mut data = connection.on::<CMsgClientMMSLobbyData>();
        let mut joined = connection.on::<CMsgClientMMSUserJoinedLobby>();
        let mut left = connection.on::<CMsgClientMMSUserLeftLobby>();
        let mut chat = connection.on::<CMsgClientMMSLobbyChatMsg>();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    Some(Ok(m)) = data.next() => (m.app_id() == app_id)
                        .then(|| LobbyEvent::Data(lobby_state(&m))),
                    Some(Ok(m)) = joined.next() => (m.app_id() == app_id).then(|| {
                        LobbyEvent::Joined {
                            lobby: m.steam_id_lobby(),
                            user: m.steam_id_user(),
                            name: m.persona_name().to_owned(),
                        }
                    }),
                    Some(Ok(m)) = left.next() => (m.app_id() == app_id).then(|| {
                        LobbyEvent::Left { lobby: m.steam_id_lobby(), user: m.steam_id_user() }
                    }),
                    Some(Ok(m)) = chat.next() => (m.app_id() == app_id).then(|| {
                        LobbyEvent::Chat {
                            lobby: m.steam_id_lobby(),
                            sender: m.steam_id_sender(),
                            message: m.lobby_message().to_vec(),
                        }
                    }),
                    () = tx.closed() => return,
                    else => return,
                };
                if let Some(event) = event
                    && tx.send(event).is_err()
                {
                    return;
                }
            }
        });
        Ok(rx)
    }
}

#[cfg(test)]
#[path = "lobbies_tests.rs"]
mod tests;
