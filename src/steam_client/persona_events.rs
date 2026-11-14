//! Persona changes for stand-in games.
//!
//! Diffs Steam's persona broadcasts.
use super::*;
use std::collections::HashMap;
use steam_vent_proto::steammessages_clientserver_friends::CMsgClientPersonaState;
use steam_vent_proto::steammessages_clientserver_friends::cmsg_client_persona_state::Friend as PersonaFriend;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

/// `EPersonaChange` bits, probe-checked.
pub mod change {
    pub const NAME: u32 = 0x0001;
    pub const STATUS: u32 = 0x0002;
    pub const COME_ONLINE: u32 = 0x0004;
    pub const GONE_OFFLINE: u32 = 0x0008;
    pub const GAME_PLAYED: u32 = 0x0010;
    pub const AVATAR: u32 = 0x0040;
    pub const RICH_PRESENCE: u32 = 0x4000;
}

/// Known persona fields of one user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PersonaSnapshot {
    pub name: Option<String>,
    pub state: Option<u32>,
    pub game: Option<u32>,
    pub avatar: Option<String>,
}

/// One change worth a callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonaEvent {
    pub steam_id: u64,
    pub flags: u32,
    /// Game the user plays, else 0.
    pub app_id: u32,
}

/// Avatar hash hex, None when unset.
fn hash_hex(hash: &[u8]) -> Option<String> {
    if hash.iter().all(|&b| b == 0) {
        return None;
    }
    Some(hash.iter().map(|b| format!("{b:02x}")).collect())
}

/// Fields present in the broadcast.
pub fn snapshot_update(old: Option<&PersonaSnapshot>, fr: &PersonaFriend) -> PersonaSnapshot {
    let mut new = old.cloned().unwrap_or_default();
    if !fr.player_name().is_empty() {
        new.name = Some(fr.player_name().to_owned());
    }
    if fr.has_persona_state() {
        new.state = Some(fr.persona_state());
    }
    if fr.has_game_played_app_id() {
        new.game = Some(fr.game_played_app_id());
    }
    if let Some(hex) = hash_hex(fr.avatar_hash()) {
        new.avatar = Some(hex);
    }
    new
}

/// `EPersonaChange` bits between snapshots.
pub fn change_flags(old: Option<&PersonaSnapshot>, new: &PersonaSnapshot) -> u32 {
    let base = PersonaSnapshot::default();
    let old = old.unwrap_or(&base);
    let mut flags = 0;
    if new.name != old.name {
        flags |= change::NAME;
    }
    if new.state != old.state {
        flags |= change::STATUS;
        let online = |s: Option<u32>| s.is_some_and(|s| s != 0);
        match (online(old.state), online(new.state)) {
            (false, true) => flags |= change::COME_ONLINE,
            (true, false) => flags |= change::GONE_OFFLINE,
            _ => {}
        }
    }
    if new.game != old.game {
        flags |= change::GAME_PLAYED;
    }
    if new.avatar != old.avatar {
        flags |= change::AVATAR;
    }
    flags
}

/// Events from one persona message.
pub fn persona_events(
    known: &mut HashMap<u64, PersonaSnapshot>,
    msg: &CMsgClientPersonaState,
) -> Vec<PersonaEvent> {
    let mut out = Vec::new();
    for fr in &msg.friends {
        let id = fr.friendid();
        let old = known.get(&id);
        let new = snapshot_update(old, fr);
        let mut flags = change_flags(old, &new);
        if !fr.rich_presence.is_empty() {
            flags |= change::RICH_PRESENCE;
        }
        let app_id = new.game.unwrap_or(0);
        known.insert(id, new);
        if flags != 0 {
            out.push(PersonaEvent {
                steam_id: id,
                flags,
                app_id,
            });
        }
    }
    out
}

impl SteamClient {
    /// Persona changes until dropped.
    pub fn persona_events(&self) -> Result<mpsc::UnboundedReceiver<PersonaEvent>> {
        let connection = self.require_connection_owned()?;
        let mut states = connection.on::<CMsgClientPersonaState>();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut known = HashMap::new();
            loop {
                tokio::select! {
                    item = states.next() => match item {
                        Some(Ok(msg)) => {
                            for event in persona_events(&mut known, &msg) {
                                if tx.send(event).is_err() {
                                    return;
                                }
                            }
                        }
                        Some(Err(_)) => {}
                        None => return,
                    },
                    () = tx.closed() => return,
                }
            }
        });
        Ok(rx)
    }
}

#[cfg(test)]
#[path = "persona_events_tests.rs"]
mod tests;
