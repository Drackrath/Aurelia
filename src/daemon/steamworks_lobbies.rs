//! Lobby commands and pushes, wire side.

use aurelia::steam_client::lobbies::{
    LobbyEvent, LobbyFilter, LobbyMember, LobbyState, LobbyUpdate, Metadata,
};

use super::steamworks::{Fields, put_blob};

/// Lobby commands; 60-69 reserved.
pub mod cmd {
    pub const CREATE: u8 = 60;
    pub const JOIN: u8 = 61;
    pub const LIST: u8 = 62;
    pub const LEAVE: u8 = 63;
    pub const SET_DATA: u8 = 64;
    pub const CHAT: u8 = 65;
    pub const REQUEST_DATA: u8 = 66;

    pub fn is_lobby(command: u8) -> bool {
        (CREATE..=REQUEST_DATA).contains(&command)
    }
}

/// Lobby push kinds.
pub mod push {
    pub const DATA: u8 = 60;
    pub const JOINED: u8 = 61;
    pub const LEFT: u8 = 62;
    pub const CHAT: u8 = 63;
}

fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn put_metadata(out: &mut Vec<u8>, meta: &Metadata) {
    out.extend_from_slice(&u32::try_from(meta.len()).unwrap_or(0).to_le_bytes());
    for (k, v) in meta {
        put_blob(out, k.as_bytes());
        put_blob(out, v.as_bytes());
    }
}

pub fn read_metadata(f: &mut Fields<'_>) -> Option<Metadata> {
    let n = f.u32()?;
    (0..n)
        .map(|_| {
            let k = String::from_utf8_lossy(f.blob()?).into_owned();
            let v = String::from_utf8_lossy(f.blob()?).into_owned();
            Some((k, v))
        })
        .collect()
}

pub fn put_state(out: &mut Vec<u8>, s: &LobbyState) {
    put_u64(out, s.lobby);
    put_u64(out, s.owner);
    for v in [s.max_members, s.lobby_type, s.flags, s.num_members] {
        put_i32(out, v);
    }
    put_metadata(out, &s.metadata);
    out.extend_from_slice(&u32::try_from(s.members.len()).unwrap_or(0).to_le_bytes());
    for m in &s.members {
        put_member(out, m);
    }
}

fn put_member(out: &mut Vec<u8>, m: &LobbyMember) {
    put_u64(out, m.steam_id);
    put_blob(out, m.name.as_bytes());
    put_metadata(out, &m.metadata);
}

/// Create body: type, max.
pub fn parse_create(body: &[u8]) -> Option<(i32, i32)> {
    let mut f = Fields::new(body);
    Some((f.i32()?, f.i32()?))
}

pub fn parse_lobby(body: &[u8]) -> Option<u64> {
    Fields::new(body).u64()
}

/// List body: count, filters.
pub fn parse_list(body: &[u8]) -> Option<(i32, Vec<LobbyFilter>)> {
    let mut f = Fields::new(body);
    let count = f.i32()?;
    let n = f.u32()?;
    let filters = (0..n)
        .map(|_| {
            Some(LobbyFilter {
                key: String::from_utf8_lossy(f.blob()?).into_owned(),
                value: String::from_utf8_lossy(f.blob()?).into_owned(),
                comparison: f.i32()?,
                filter_type: f.i32()?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some((count, filters))
}

pub fn parse_update(body: &[u8]) -> Option<LobbyUpdate> {
    let mut f = Fields::new(body);
    Some(LobbyUpdate {
        lobby: f.u64()?,
        member: f.u64()?,
        max_members: f.i32()?,
        lobby_type: f.i32()?,
        flags: f.i32()?,
        metadata: read_metadata(&mut f)?,
    })
}

/// Chat body: lobby, message bytes.
pub fn parse_chat(body: &[u8]) -> Option<(u64, Vec<u8>)> {
    let mut f = Fields::new(body);
    Some((f.u64()?, f.rest().to_vec()))
}

pub fn created_body(eresult: i32, lobby: u64) -> Vec<u8> {
    let mut out = Vec::new();
    put_i32(&mut out, eresult);
    put_u64(&mut out, lobby);
    out
}

pub fn joined_body(enter_response: i32, state: &LobbyState) -> Vec<u8> {
    let mut out = Vec::new();
    put_i32(&mut out, enter_response);
    put_state(&mut out, state);
    out
}

pub fn list_body(eresult: i32, lobbies: &[LobbyState]) -> Vec<u8> {
    let mut out = Vec::new();
    put_i32(&mut out, eresult);
    out.extend_from_slice(&u32::try_from(lobbies.len()).unwrap_or(0).to_le_bytes());
    for l in lobbies {
        put_state(&mut out, l);
    }
    out
}

/// Push payload for an event.
pub fn event_payload(event: &LobbyEvent) -> Vec<u8> {
    let mut out = Vec::new();
    match event {
        LobbyEvent::Data(state) => {
            out.push(push::DATA);
            put_state(&mut out, state);
        }
        LobbyEvent::Joined { lobby, user, name } => {
            out.push(push::JOINED);
            put_u64(&mut out, *lobby);
            put_u64(&mut out, *user);
            put_blob(&mut out, name.as_bytes());
        }
        LobbyEvent::Left { lobby, user } => {
            out.push(push::LEFT);
            put_u64(&mut out, *lobby);
            put_u64(&mut out, *user);
        }
        LobbyEvent::Chat {
            lobby,
            sender,
            message,
        } => {
            out.push(push::CHAT);
            put_u64(&mut out, *lobby);
            put_u64(&mut out, *sender);
            out.extend_from_slice(message);
        }
    }
    out
}
