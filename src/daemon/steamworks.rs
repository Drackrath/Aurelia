//! IPC for the stand-in steamclient.so.
//!
//! Frames reuse [`proto`]. Fields are little-endian.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use aurelia::core::models::DownloadState;
use aurelia::steam_client::auth_ticket::{AuthSessionTicket, TicketAuthEvent, TicketKind};
use aurelia::steam_client::avatars::{Avatar, AvatarSize, fetch_avatar};
use aurelia::steam_client::microtxn::{self, MicroTxnRequest};
use aurelia::steam_client::persona_events::{PersonaEvent, change};
use aurelia::steam_client::playing::{self, GAMES, Launch};
use aurelia::steam_client::remote_storage::{RemoteStorageInfo, remote_storage_info};
use aurelia::steam_client::steamworks_info::{AppInstallInfo, DlcInfo};
use aurelia::steam_client::user_stats::{StoreOutcome, UserStats};
use aurelia::steam_client::workshop_ugc::{UgcDetails, UgcInstall, UgcPage, UgcUpdate};
use aurelia::steam_client::{EncryptedTicketOutcome, Friend, SteamClient};

use super::proto::{self, SharedWriter};
use super::steamworks_lobbies as lobby;
use aurelia::steam_client::lobbies::{
    JoinedLobby, LobbyEvent, LobbyFilter, LobbyState, LobbyUpdate,
};

/// Bumped on incompatible changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// Welcome status codes.
pub mod welcome {
    pub const OK: u8 = 0;
    pub const NO_SESSION: u8 = 1;
    pub const REFUSED: u8 = 2;
}

/// Response status codes.
pub mod status {
    pub const OK: u8 = 0;
    pub const FAILED: u8 = 1;
    pub const NO_SESSION: u8 = 2;
    pub const UNSUPPORTED: u8 = 3;
    pub const MALFORMED: u8 = 4;
}

/// Request commands.
pub mod cmd {
    pub const STATUS: u8 = 1;
    pub const OWNERSHIP_TICKET: u8 = 2;
    pub const CREATE_AUTH_TICKET: u8 = 3;
    pub const CANCEL_AUTH_TICKET: u8 = 4;
    pub const ENCRYPTED_APP_TICKET: u8 = 5;
    pub const DLC_LIST: u8 = 6;
    pub const APP_INFO: u8 = 7;
    pub const SESSION_INFO: u8 = 8;
    /// Cloud folder and flags; 40-49 reserved.
    pub const CLOUD_INFO: u8 = 40;
    /// Stats range is 30-39.
    pub const USER_STATS: u8 = 30;
    pub const STORE_STATS: u8 = 31;
    pub const FRIENDS: u8 = 50;
    pub const SET_RICH_PRESENCE: u8 = 51;
    pub const FRIEND_RICH_PRESENCE: u8 = 52;
    pub const OPEN_URL: u8 = 53;
    /// Workshop range is 70-79.
    pub const UGC_QUERY: u8 = 70;
    pub const UGC_SUBSCRIBE: u8 = 71;
    pub const UGC_UNSUBSCRIBE: u8 = 72;
    pub const UGC_SUBSCRIBED: u8 = 73;
    pub const UGC_ITEM_STATE: u8 = 74;
    pub const UGC_DOWNLOAD: u8 = 75;
    pub const UGC_CREATE: u8 = 76;
    pub const UGC_UPDATE: u8 = 77;
    /// Avatars; 90-94 reserved.
    pub const AVATAR: u8 = 90;
    /// Invites range is 95-99.
    pub const GAME_INVITE: u8 = 95;
    pub const LOBBY_INVITE: u8 = 96;
}

/// Cloud info flag bits.
pub mod cloud_flag {
    pub const ENABLED: u8 = 1;
    pub const WRITABLE: u8 = 2;
}

/// `UGC_QUERY` kinds.
pub mod ugc_kind {
    pub const ALL_PAGE: u8 = 0;
    pub const ALL_CURSOR: u8 = 1;
    pub const DETAILS: u8 = 2;
    pub const SUBSCRIBED: u8 = 3;
    pub const USER: u8 = 4;
}

/// `UGC_UPDATE` field bits.
pub mod ugc_update {
    pub const TITLE: u8 = 1;
    pub const DESCRIPTION: u8 = 2;
    pub const VISIBILITY: u8 = 4;
    pub const TAGS: u8 = 8;
}

/// `EUserUGCList_Published`.
pub const UGC_LIST_PUBLISHED: u32 = 0;

/// `UGC_ITEM_STATE` flag bits.
pub mod ugc_flag {
    pub const INSTALLED: u8 = 1;
    pub const NEEDS_UPDATE: u8 = 2;
    pub const DOWNLOADING: u8 = 4;
}

/// Subscribed items per page.
pub const UGC_PAGE_SIZE: usize = 50;

/// User stats flag bits.
pub mod stats_flag {
    /// Config allows stats writes.
    pub const WRITES: u8 = 1;
}

/// Rich presence limits Steam enforces.
pub mod rich_presence {
    pub const MAX_KEYS: usize = 30;
    pub const MAX_KEY_LEN: usize = 64;
    pub const MAX_VALUE_LEN: usize = 256;
}

/// `OWNERSHIP_TICKET` request flags.
pub mod ownership_flag {
    /// Append the owner SteamID.
    pub const WITH_OWNER: u8 = 1;
}

/// Ticket then owner; 0 is unknown.
pub fn ticket_with_owner(mut ticket: Vec<u8>, owner: Option<u64>) -> Vec<u8> {
    ticket.extend_from_slice(&owner.unwrap_or(0).to_le_bytes());
    ticket
}

/// Per-DLC flag bits.
pub mod dlc_flag {
    /// Has a store page.
    pub const AVAILABLE: u8 = 1;
    /// Depots on disk or none.
    pub const CONTENT_READY: u8 = 2;
    pub const DISABLED: u8 = 4;
}

/// Pushed message kinds.
pub mod push {
    pub const TICKET_AUTH: u8 = 1;
    pub const MICROTXN_PENDING: u8 = 80;
    pub const MICROTXN_RESULT: u8 = 81;
    /// Workshop download finished.
    pub const UGC_DOWNLOADED: u8 = 70;
    /// Join a friend by connect string.
    pub const JOIN_CONNECT: u8 = 95;
    /// Join a friend's lobby.
    pub const JOIN_LOBBY: u8 = 96;
    /// Persona pushes; 100-104 reserved.
    pub const PERSONA_CHANGE: u8 = 100;
    pub const RICH_PRESENCE: u8 = 101;
}

/// Push payloads for one persona event.
pub fn persona_payloads(event: &PersonaEvent) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let other = event.flags & !change::RICH_PRESENCE;
    if other != 0 {
        let mut p = vec![push::PERSONA_CHANGE];
        p.extend_from_slice(&event.steam_id.to_le_bytes());
        p.extend_from_slice(&other.to_le_bytes());
        out.push(p);
    }
    if event.flags & change::RICH_PRESENCE != 0 {
        let mut p = vec![push::RICH_PRESENCE];
        p.extend_from_slice(&event.steam_id.to_le_bytes());
        p.extend_from_slice(&event.app_id.to_le_bytes());
        out.push(p);
    }
    out
}

/// Auth ticket kinds on the wire.
pub mod ticket_kind {
    pub const SESSION: u8 = 0;
    pub const WEB_API: u8 = 1;
}

/// Polls ticket verdicts this often.
const EVENT_POLL: Duration = Duration::from_millis(250);

/// Little-endian field reader.
pub struct Fields<'a>(&'a [u8]);

impl<'a> Fields<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self(bytes)
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Some(head)
    }

    pub fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    pub fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> Option<i32> {
        self.take(4)
            .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }

    /// u32 length, then bytes.
    pub fn blob(&mut self) -> Option<&'a [u8]> {
        let len = usize::try_from(self.u32()?).ok()?;
        self.take(len)
    }

    pub fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.0)
    }
}

/// Appends a length-prefixed blob.
pub fn put_blob(out: &mut Vec<u8>, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&bytes[..len as usize]);
}

/// Opening message from the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hello {
    pub version: u32,
    pub app_id: u32,
    pub pid: u32,
}

impl Hello {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let mut f = Fields::new(bytes);
        Some(Self {
            version: f.u32()?,
            app_id: f.u32()?,
            pid: f.u32()?,
        })
    }
}

/// One request from the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: u32,
    pub app_id: u32,
    pub pipe: i32,
    pub user: i32,
    pub command: u8,
    pub body: Vec<u8>,
}

impl Request {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let mut f = Fields::new(bytes);
        Some(Self {
            id: f.u32()?,
            app_id: f.u32()?,
            pipe: f.i32()?,
            user: f.i32()?,
            command: f.u8()?,
            body: f.rest().to_vec(),
        })
    }
}

/// Welcome payload.
pub fn welcome_payload(code: u8, steam_id: u64, message: &str) -> Vec<u8> {
    let mut out = vec![code];
    out.extend_from_slice(&steam_id.to_le_bytes());
    put_blob(&mut out, message.as_bytes());
    out
}

/// Response payload.
pub fn response_payload(id: u32, code: u8, body: &[u8]) -> Vec<u8> {
    let mut out = id.to_le_bytes().to_vec();
    out.push(code);
    out.extend_from_slice(body);
    out
}

/// Ticket verdict push payload.
pub fn ticket_auth_payload(handle: u32, event: &TicketAuthEvent) -> Vec<u8> {
    let mut out = vec![push::TICKET_AUTH];
    out.extend_from_slice(&handle.to_le_bytes());
    out.extend_from_slice(&event.steam_id.to_le_bytes());
    out.extend_from_slice(&event.owner_steam_id.to_le_bytes());
    out.extend_from_slice(&event.game_id.to_le_bytes());
    out.extend_from_slice(&event.ticket_crc.to_le_bytes());
    out.extend_from_slice(&event.estate.to_le_bytes());
    out.extend_from_slice(&event.response.to_le_bytes());
    out
}

/// Auth ticket response body.
pub fn auth_ticket_body(ticket: &AuthSessionTicket) -> Vec<u8> {
    let mut out = ticket.handle.to_le_bytes().to_vec();
    out.extend_from_slice(&ticket.crc.to_le_bytes());
    out.push(u8::from(ticket.acknowledged));
    put_blob(&mut out, &ticket.ticket);
    out
}

/// `EResult` OK.
const ERESULT_OK: i32 = 1;

/// Encrypted ticket body: eresult, blob.
pub fn encrypted_ticket_body(outcome: &EncryptedTicketOutcome) -> Vec<u8> {
    let (eresult, ticket): (i32, &[u8]) = match outcome {
        EncryptedTicketOutcome::Issued(t) => (ERESULT_OK, t),
        EncryptedTicketOutcome::Refused(e) => (*e, &[]),
    };
    let mut out = eresult.to_le_bytes().to_vec();
    put_blob(&mut out, ticket);
    out
}

/// DLC list body.
pub fn dlc_list_body(dlcs: &[DlcInfo]) -> Vec<u8> {
    let count = u32::try_from(dlcs.len()).unwrap_or(u32::MAX);
    let mut out = count.to_le_bytes().to_vec();
    for dlc in dlcs.iter().take(count as usize) {
        out.extend_from_slice(&dlc.app_id.to_le_bytes());
        let mut flags = 0;
        if dlc.name.is_some() {
            flags |= dlc_flag::AVAILABLE;
        }
        if dlc.content_ready {
            flags |= dlc_flag::CONTENT_READY;
        }
        if dlc.disabled {
            flags |= dlc_flag::DISABLED;
        }
        out.push(flags);
        put_blob(&mut out, dlc.name.as_deref().unwrap_or_default().as_bytes());
    }
    out
}

/// App info body: build, folder, language.
pub fn app_info_body(info: &AppInstallInfo) -> Vec<u8> {
    let mut out = info.build_id.to_le_bytes().to_vec();
    let dir = info
        .install_dir
        .as_ref()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    put_blob(&mut out, dir.as_bytes());
    put_blob(
        &mut out,
        info.language.as_deref().unwrap_or_default().as_bytes(),
    );
    out
}

/// Session body: server time, country.
pub fn session_info_body(server_time: u32, country: &str) -> Vec<u8> {
    let mut out = server_time.to_le_bytes().to_vec();
    put_blob(&mut out, country.as_bytes());
    out
}

/// Cloud info response body.
pub fn cloud_info_body(info: &RemoteStorageInfo) -> Vec<u8> {
    let mut flags = 0;
    if info.enabled {
        flags |= cloud_flag::ENABLED;
    }
    if info.writable {
        flags |= cloud_flag::WRITABLE;
    }
    let mut out = vec![flags];
    put_blob(&mut out, info.remote_dir.to_string_lossy().as_bytes());
    out
}

/// User stats body; see the .so.
pub fn user_stats_body(stats: &UserStats, writes: bool) -> Vec<u8> {
    let mut out = stats.eresult.to_le_bytes().to_vec();
    out.extend_from_slice(&stats.crc.to_le_bytes());
    out.push(if writes { stats_flag::WRITES } else { 0 });
    let len = |n: usize| u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes();
    out.extend_from_slice(&len(stats.stats.len()));
    for stat in &stats.stats {
        out.extend_from_slice(&stat.id.to_le_bytes());
        out.push(stat.kind);
        out.extend_from_slice(&stat.value.to_le_bytes());
        put_blob(&mut out, stat.name.as_bytes());
    }
    out.extend_from_slice(&len(stats.achievements.len()));
    for ach in &stats.achievements {
        out.extend_from_slice(&ach.block.to_le_bytes());
        out.push(ach.bit);
        out.extend_from_slice(&ach.unlock_time.to_le_bytes());
        out.push(u8::from(ach.hidden));
        put_blob(&mut out, ach.name.as_bytes());
        put_blob(&mut out, ach.display_name.as_bytes());
        put_blob(&mut out, ach.description.as_bytes());
    }
    out
}

/// Own persona plus roster.
#[derive(Debug, Clone, Default)]
pub struct FriendsSnapshot {
    pub name: String,
    pub state: u32,
    pub friends: Vec<Friend>,
}

/// Friends body: self, then entries.
pub fn friends_body(snapshot: &FriendsSnapshot) -> Vec<u8> {
    let mut out = Vec::new();
    put_blob(&mut out, snapshot.name.as_bytes());
    out.extend_from_slice(&snapshot.state.to_le_bytes());
    let count = u32::try_from(snapshot.friends.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&count.to_le_bytes());
    for f in snapshot.friends.iter().take(count as usize) {
        out.extend_from_slice(&f.steam_id.to_le_bytes());
        out.extend_from_slice(&f.relationship.to_le_bytes());
        out.extend_from_slice(&f.persona_state.unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&f.game_app_id.unwrap_or(0).to_le_bytes());
        put_blob(
            &mut out,
            f.persona_name.as_deref().unwrap_or_default().as_bytes(),
        );
    }
    out
}

/// Stat ID and raw value pairs.
pub type StatChanges = Vec<(u32, u32)>;

/// Store request: crc, reset, pairs.
pub fn parse_store_request(body: &[u8]) -> Option<(u32, bool, StatChanges)> {
    let mut f = Fields::new(body);
    let crc = f.u32()?;
    let reset = f.u8()? != 0;
    let count = f.u32()?;
    // Bounded by the body size.
    if usize::try_from(count).ok()? > body.len() / 8 {
        return None;
    }
    let mut changes = Vec::new();
    for _ in 0..count {
        changes.push((f.u32()?, f.u32()?));
    }
    f.rest().is_empty().then_some((crc, reset, changes))
}

/// Store reply: eresult, crc, reverted.
pub fn store_outcome_body(outcome: &StoreOutcome) -> Vec<u8> {
    let mut out = outcome.eresult.to_le_bytes().to_vec();
    out.extend_from_slice(&outcome.crc.to_le_bytes());
    let count = u32::try_from(outcome.reverted.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&count.to_le_bytes());
    for (id, value) in &outcome.reverted {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// Key/value list body.
pub fn pairs_body(pairs: &[(String, String)]) -> Vec<u8> {
    let count = u32::try_from(pairs.len()).unwrap_or(u32::MAX);
    let mut out = count.to_le_bytes().to_vec();
    for (key, value) in pairs.iter().take(count as usize) {
        put_blob(&mut out, key.as_bytes());
        put_blob(&mut out, value.as_bytes());
    }
    out
}

/// A game's Workshop query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UgcQuery {
    All {
        query_type: u32,
        page: u32,
        cursor: Option<String>,
        search: String,
        tags: Vec<String>,
        excluded: Vec<String>,
    },
    Details(Vec<u64>),
    Subscribed {
        page: u32,
    },
    /// An account's own list.
    User {
        account: u32,
        list: u32,
        page: u32,
        tags: Vec<String>,
        excluded: Vec<String>,
    },
}

impl UgcQuery {
    pub fn parse(body: &[u8]) -> Option<Self> {
        let mut f = Fields::new(body);
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let kind = f.u8()?;
        let query_type = f.u32()?;
        let page = f.u32()?;
        let cursor = text(f.blob()?);
        let search = text(f.blob()?);
        let tag_count = usize::try_from(f.u32()?).ok()?;
        if tag_count > body.len() {
            return None;
        }
        let mut tags = Vec::new();
        for _ in 0..tag_count {
            tags.push(text(f.blob()?));
        }
        let id_count = usize::try_from(f.u32()?).ok()?;
        if id_count > body.len() / 8 {
            return None;
        }
        let mut ids = Vec::new();
        for _ in 0..id_count {
            ids.push(f.u64()?);
        }
        // Newer clients append these.
        let mut excluded = Vec::new();
        let (mut account, mut list) = (0, 0);
        if let Some(count) = f.u32() {
            let count = usize::try_from(count).ok()?;
            if count > body.len() {
                return None;
            }
            for _ in 0..count {
                excluded.push(text(f.blob()?));
            }
            account = f.u32()?;
            list = f.u32()?;
        }
        Some(match kind {
            ugc_kind::ALL_PAGE | ugc_kind::ALL_CURSOR => Self::All {
                query_type,
                page,
                cursor: (kind == ugc_kind::ALL_CURSOR).then_some(cursor),
                search,
                tags,
                excluded,
            },
            ugc_kind::DETAILS => Self::Details(ids),
            ugc_kind::SUBSCRIBED => Self::Subscribed { page },
            ugc_kind::USER => Self::User {
                account,
                list,
                page,
                tags,
                excluded,
            },
            _ => return None,
        })
    }
}

/// One item on the wire.
pub fn put_ugc_item(out: &mut Vec<u8>, d: &UgcDetails) {
    out.extend_from_slice(&d.id.to_le_bytes());
    out.extend_from_slice(&d.result.to_le_bytes());
    for v in [d.file_type, d.creator_app, d.consumer_app] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    put_blob(out, d.title.as_bytes());
    put_blob(out, d.description.as_bytes());
    out.extend_from_slice(&d.owner.to_le_bytes());
    for v in [d.created, d.updated, d.visibility] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.push(u8::from(d.banned));
    out.push(u8::from(d.accepted));
    put_blob(out, d.tags.as_bytes());
    out.extend_from_slice(&d.file.to_le_bytes());
    out.extend_from_slice(&d.preview_file.to_le_bytes());
    put_blob(out, d.file_name.as_bytes());
    out.extend_from_slice(&d.file_size.to_le_bytes());
    out.extend_from_slice(&d.preview_size.to_le_bytes());
    put_blob(out, d.url.as_bytes());
    for v in [d.votes_up, d.votes_down, d.score.to_bits(), d.children] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    put_blob(out, d.metadata.as_bytes());
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    out.extend_from_slice(&count(d.kv_tags.len()).to_le_bytes());
    for (key, value) in d.kv_tags.iter().take(count(d.kv_tags.len()) as usize) {
        put_blob(out, key.as_bytes());
        put_blob(out, value.as_bytes());
    }
    out.extend_from_slice(&count(d.previews.len()).to_le_bytes());
    for p in d.previews.iter().take(count(d.previews.len()) as usize) {
        put_blob(out, p.url.as_bytes());
        put_blob(out, p.file_name.as_bytes());
        out.extend_from_slice(&p.kind.to_le_bytes());
    }
}

/// Parses a `UGC_UPDATE` body.
pub fn parse_ugc_update(body: &[u8]) -> Option<UgcUpdate> {
    let mut f = Fields::new(body);
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let id = f.u64()?;
    let fields = f.u8()?;
    let title = text(f.blob()?);
    let description = text(f.blob()?);
    let visibility = f.u32()?;
    let count = usize::try_from(f.u32()?).ok()?;
    if count > body.len() || id == 0 {
        return None;
    }
    let mut tags = Vec::with_capacity(count);
    for _ in 0..count {
        tags.push(text(f.blob()?));
    }
    let has = |bit: u8| fields & bit != 0;
    Some(UgcUpdate {
        id,
        title: has(ugc_update::TITLE).then_some(title),
        description: has(ugc_update::DESCRIPTION).then_some(description),
        visibility: has(ugc_update::VISIBILITY).then_some(visibility),
        tags: has(ugc_update::TAGS).then_some(tags),
    })
}

/// Query page body.
pub fn ugc_page_body(page: &UgcPage) -> Vec<u8> {
    let mut out = page.total.to_le_bytes().to_vec();
    put_blob(&mut out, page.next_cursor.as_bytes());
    let count = u32::try_from(page.items.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&count.to_le_bytes());
    for item in page.items.iter().take(count as usize) {
        put_ugc_item(&mut out, item);
    }
    out
}

/// Parses a key/value list within limits.
pub fn parse_pairs(f: &mut Fields) -> Option<Vec<(String, String)>> {
    let count = usize::try_from(f.u32()?).ok()?;
    if count > rich_presence::MAX_KEYS {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let key = std::str::from_utf8(f.blob()?).ok()?;
        let value = std::str::from_utf8(f.blob()?).ok()?;
        let fits = !key.is_empty()
            && key.len() <= rich_presence::MAX_KEY_LEN
            && value.len() <= rich_presence::MAX_VALUE_LEN;
        if !fits || out.iter().any(|(k, _): &(String, String)| k == key) {
            return None;
        }
        out.push((key.to_string(), value.to_string()));
    }
    Some(out)
}

/// Web pages only.
pub fn openable_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// Item state body.
pub fn ugc_state_body(state: &UgcInstall, progress: Option<(u64, u64)>) -> Vec<u8> {
    let mut flags = 0;
    if state.installed {
        flags |= ugc_flag::INSTALLED;
    }
    if state.needs_update {
        flags |= ugc_flag::NEEDS_UPDATE;
    }
    if progress.is_some() {
        flags |= ugc_flag::DOWNLOADING;
    }
    let mut out = vec![flags];
    out.extend_from_slice(&state.size.to_le_bytes());
    out.extend_from_slice(&state.timestamp.to_le_bytes());
    put_blob(&mut out, state.folder.as_bytes());
    let (done, total) = progress.unwrap_or_default();
    out.extend_from_slice(&done.to_le_bytes());
    out.extend_from_slice(&total.to_le_bytes());
    out
}

/// Download finished push.
pub fn ugc_downloaded_payload(app_id: u32, id: u64, result: i32) -> Vec<u8> {
    let mut out = vec![push::UGC_DOWNLOADED];
    out.extend_from_slice(&app_id.to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&result.to_le_bytes());
    out
}

/// Downloads in flight, per item.
type Downloads = Mutex<HashMap<(u32, u64), Arc<std::sync::RwLock<DownloadState>>>>;

static UGC_DOWNLOADS: std::sync::LazyLock<Downloads> = std::sync::LazyLock::new(Downloads::default);

/// Bytes done and total.
fn ugc_progress(app_id: u32, id: u64) -> Option<(u64, u64)> {
    let downloads = UGC_DOWNLOADS.lock().unwrap();
    let state = downloads.get(&(app_id, id))?.read().ok()?;
    Some((state.downloaded_bytes, state.total_bytes))
}

/// `EResult` for a failed download.
const ERESULT_FAIL: i32 = 2;

/// Avatar body: width, height, RGBA.
pub fn avatar_body(avatar: &Avatar) -> Vec<u8> {
    let mut out = avatar.width.to_le_bytes().to_vec();
    out.extend_from_slice(&avatar.height.to_le_bytes());
    out.extend_from_slice(&avatar.rgba);
    out
}

/// Wait for a persona with avatar.
const AVATAR_WAIT: Duration = Duration::from_secs(3);

/// What the daemon offers games.
pub trait Backend: Send + Sync + 'static {
    /// SteamID when logged on.
    fn steam_id(&self) -> impl Future<Output = Option<u64>> + Send;
    /// Report playing; returns launch id.
    fn start(&self, app_id: u32, pid: Option<u32>) -> impl Future<Output = Result<u64>> + Send;
    /// Clear one launch.
    fn stop(&self, launch: u64) -> impl Future<Output = ()> + Send;
    fn ownership_ticket(&self, app_id: u32) -> impl Future<Output = Result<Vec<u8>>> + Send;
    /// Lender under Family Sharing, else us.
    fn app_owner(&self, app_id: u32) -> impl Future<Output = Result<Option<u64>>> + Send;
    /// Stored account while Steam is unreachable.
    fn offline_steam_id(&self) -> impl Future<Output = Option<u64>> + Send;
    /// Valid cached ticket only, never Steam.
    fn cached_ownership_ticket(
        &self,
        steam_id: u64,
        app_id: u32,
    ) -> impl Future<Output = Result<Vec<u8>>> + Send;
    fn create_auth_ticket(
        &self,
        app_id: u32,
        pipe: u32,
        kind: TicketKind,
    ) -> impl Future<Output = Result<AuthSessionTicket>> + Send;
    fn cancel_auth_ticket(&self, handle: u32) -> impl Future<Output = Result<bool>> + Send;
    /// Drains Steam's ticket verdicts.
    fn ticket_events(&self) -> impl Future<Output = Vec<TicketAuthEvent>> + Send;
    fn encrypted_app_ticket(
        &self,
        app_id: u32,
        userdata: Vec<u8>,
    ) -> impl Future<Output = Result<EncryptedTicketOutcome>> + Send;
    /// DLC from PICS, store, manifest.
    fn dlcs(&self, app_id: u32) -> impl Future<Output = Result<Vec<DlcInfo>>> + Send;
    /// Language resolved, never empty.
    fn app_info(&self, app_id: u32) -> impl Future<Output = Result<AppInstallInfo>> + Send;
    /// Server time and logon country.
    fn session_info(&self) -> impl Future<Output = Result<(u32, String)>> + Send;
    /// Cloud folder and switches.
    fn cloud_info(
        &self,
        steam_id: u64,
        app_id: u32,
    ) -> impl Future<Output = Result<RemoteStorageInfo>> + Send;
    /// Lobby calls; see `steam_client::lobbies`.
    fn create_lobby(
        &self,
        app_id: u32,
        lobby_type: i32,
        max: i32,
    ) -> impl Future<Output = Result<(i32, u64)>> + Send;
    fn join_lobby(
        &self,
        app_id: u32,
        lobby: u64,
    ) -> impl Future<Output = Result<JoinedLobby>> + Send;
    fn leave_lobby(&self, app_id: u32, lobby: u64) -> impl Future<Output = Result<i32>> + Send;
    fn lobby_list(
        &self,
        app_id: u32,
        count: i32,
        filters: Vec<LobbyFilter>,
    ) -> impl Future<Output = Result<(i32, Vec<LobbyState>)>> + Send;
    fn set_lobby_data(
        &self,
        app_id: u32,
        update: LobbyUpdate,
    ) -> impl Future<Output = Result<()>> + Send;
    fn send_lobby_chat(
        &self,
        app_id: u32,
        lobby: u64,
        message: Vec<u8>,
    ) -> impl Future<Output = Result<()>> + Send;
    fn request_lobby_data(
        &self,
        app_id: u32,
        lobby: u64,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Friends' persona changes.
    fn persona_events(
        &self,
    ) -> impl Future<Output = Option<mpsc::UnboundedReceiver<PersonaEvent>>> + Send;
    /// Lobby pushes for this app.
    fn lobby_events(
        &self,
        app_id: u32,
    ) -> impl Future<Output = Option<mpsc::UnboundedReceiver<LobbyEvent>>> + Send;
    fn ugc_query(
        &self,
        app_id: u32,
        query: UgcQuery,
    ) -> impl Future<Output = Result<UgcPage>> + Send;
    fn ugc_subscribe(
        &self,
        app_id: u32,
        id: u64,
        subscribe: bool,
    ) -> impl Future<Output = Result<()>> + Send;
    fn ugc_subscribed(&self, app_id: u32) -> impl Future<Output = Result<Vec<u64>>> + Send;
    /// Config switch for Workshop publishing.
    fn workshop_publishing(&self) -> impl Future<Output = bool> + Send;
    /// New empty item; its ID.
    fn ugc_create(&self, app_id: u32, file_type: u32) -> impl Future<Output = Result<u64>> + Send;
    fn ugc_update(&self, app_id: u32, update: UgcUpdate)
    -> impl Future<Output = Result<()>> + Send;
    fn ugc_state(&self, app_id: u32, id: u64) -> impl Future<Output = Result<UgcInstall>> + Send;
    /// Downloads and waits.
    fn ugc_download(
        &self,
        app_id: u32,
        id: u64,
        progress: Arc<std::sync::RwLock<DownloadState>>,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Stats of `steam_id`, else ours.
    fn user_stats(
        &self,
        app_id: u32,
        steam_id: Option<u64>,
    ) -> impl Future<Output = Result<UserStats>> + Send;
    /// Config switch for stats writes.
    fn stats_writes(&self) -> impl Future<Output = bool> + Send;
    fn store_stats(
        &self,
        app_id: u32,
        crc: u32,
        reset: bool,
        changes: Vec<(u32, u32)>,
    ) -> impl Future<Output = Result<StoreOutcome>> + Send;
    /// Install facts from local data.
    fn offline_app_info(&self, app_id: u32) -> impl Future<Output = Result<AppInstallInfo>> + Send;
    /// Saved DLC catalog, if any.
    fn offline_dlcs(
        &self,
        app_id: u32,
    ) -> impl Future<Output = Result<Option<Vec<DlcInfo>>>> + Send;
    /// Own persona and friends roster.
    fn friends(&self) -> impl Future<Output = Result<FriendsSnapshot>> + Send;
    /// Replace the app's rich presence.
    fn set_rich_presence(
        &self,
        app_id: u32,
        pairs: Vec<(String, String)>,
    ) -> impl Future<Output = Result<()>> + Send;
    /// A user's rich presence.
    fn friend_rich_presence(
        &self,
        steam_id: u64,
    ) -> impl Future<Output = Result<Vec<(String, String)>>> + Send;
    /// Opens a web page, if possible.
    fn open_url(&self, url: String) -> impl Future<Output = Result<bool>> + Send;
    /// A user's avatar, decoded.
    fn avatar(
        &self,
        steam_id: u64,
        size: AvatarSize,
    ) -> impl Future<Output = Result<Arc<Avatar>>> + Send;
    /// Steam game invite to a friend.
    fn game_invite(
        &self,
        app_id: u32,
        friend: u64,
        connect: String,
    ) -> impl Future<Output = Result<()>> + Send;
    fn lobby_invite(
        &self,
        app_id: u32,
        lobby: u64,
        friend: u64,
    ) -> impl Future<Output = Result<()>> + Send;
}

/// Where a join goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinTarget {
    Lobby(u64),
    Connect(String),
}

/// Join request push payload.
pub fn join_payload(friend: u64, target: &JoinTarget) -> Vec<u8> {
    match target {
        JoinTarget::Lobby(lobby) => {
            let mut out = vec![push::JOIN_LOBBY];
            out.extend_from_slice(&lobby.to_le_bytes());
            out.extend_from_slice(&friend.to_le_bytes());
            out
        }
        JoinTarget::Connect(connect) => {
            let mut out = vec![push::JOIN_CONNECT];
            out.extend_from_slice(&friend.to_le_bytes());
            put_blob(&mut out, connect.as_bytes());
            out
        }
    }
}

/// Games of `app_id` told.
pub fn join_running(router: &Router, app_id: u32, friend: u64, target: &JoinTarget) -> usize {
    router.push_to_app(app_id, &join_payload(friend, target))
}

/// Pending purchase push payload.
pub fn microtxn_pending_payload(request: &MicroTxnRequest) -> Vec<u8> {
    let mut out = vec![push::MICROTXN_PENDING];
    out.extend_from_slice(&request.app_id.to_le_bytes());
    out.extend_from_slice(&request.order_id.to_le_bytes());
    put_blob(&mut out, request.approval_url().as_bytes());
    out
}

/// Purchase decision push payload.
pub fn microtxn_result_payload(request: &MicroTxnRequest, authorized: bool) -> Vec<u8> {
    let mut out = vec![push::MICROTXN_RESULT];
    out.extend_from_slice(&request.app_id.to_le_bytes());
    out.extend_from_slice(&request.order_id.to_le_bytes());
    out.push(u8::from(authorized));
    out
}

/// One connected online game.
struct GameLink {
    conn: u64,
    app_id: u32,
    tx: mpsc::UnboundedSender<Vec<u8>>,
}

/// Routes pushes to owning connections.
#[derive(Default)]
pub struct Router {
    owners: Mutex<HashMap<u32, mpsc::UnboundedSender<Vec<u8>>>>,
    games: Mutex<Vec<GameLink>>,
}

impl Router {
    fn register_game(&self, conn: u64, app_id: u32, tx: mpsc::UnboundedSender<Vec<u8>>) {
        self.games
            .lock()
            .unwrap()
            .push(GameLink { conn, app_id, tx });
    }

    fn unregister_game(&self, conn: u64) {
        self.games.lock().unwrap().retain(|g| g.conn != conn);
    }

    /// Sends to every game of `app_id`.
    pub fn push_to_app(&self, app_id: u32, payload: &[u8]) -> usize {
        let games = self.games.lock().unwrap();
        games
            .iter()
            .filter(|g| g.app_id == app_id)
            .filter(|g| g.tx.send(payload.to_vec()).is_ok())
            .count()
    }

    fn register(&self, handle: u32, tx: mpsc::UnboundedSender<Vec<u8>>) {
        self.owners.lock().unwrap().insert(handle, tx);
    }

    fn unregister(&self, handle: u32) {
        self.owners.lock().unwrap().remove(&handle);
    }

    pub fn is_empty(&self) -> bool {
        self.owners.lock().unwrap().is_empty()
    }

    /// Sends events to ticket owners.
    pub fn dispatch(&self, events: Vec<TicketAuthEvent>) {
        let owners = self.owners.lock().unwrap();
        for event in events {
            let Some(handle) = event.handle else { continue };
            if let Some(tx) = owners.get(&handle) {
                let _ = tx.send(ticket_auth_payload(handle, &event));
            }
        }
    }
}

/// Tell the game; open the checkout.
pub fn microtxn_arrived(router: &Router, request: &MicroTxnRequest) {
    let url = request.approval_url();
    let games = router.push_to_app(request.app_id, &microtxn_pending_payload(request));
    tracing::warn!(
        app_id = request.app_id,
        order = request.order_id,
        games,
        "steamworks: purchase needs approval at {url}; afterwards run \
         `aurelia microtxn approve {}` (or `deny`)",
        request.order_id
    );
    spawn_browser(&url);
}

/// Report the decision to the game.
pub fn microtxn_decide(
    router: &Router,
    order_id: u64,
    authorized: bool,
) -> Result<MicroTxnRequest> {
    let request = microtxn::take(order_id)
        .ok_or_else(|| anyhow!("no pending purchase with order {order_id}"))?;
    let payload = microtxn_result_payload(&request, authorized);
    if router.push_to_app(request.app_id, &payload) == 0 {
        // Keep it for a later retry.
        microtxn::record(request.clone());
        return Err(anyhow!("app {} is not connected", request.app_id));
    }
    Ok(request)
}

/// Polls verdicts while tickets exist.
pub async fn pump_events<B: Backend>(backend: Arc<B>, router: Arc<Router>) {
    loop {
        tokio::time::sleep(EVENT_POLL).await;
        if !router.is_empty() {
            router.dispatch(backend.ticket_events().await);
        }
    }
}

/// Per-connection shared context.
struct Conn<B> {
    backend: Arc<B>,
    router: Arc<Router>,
    app_id: u32,
    /// Offline account; cache-only answers.
    offline: Option<u64>,
    owned: Mutex<HashSet<u32>>,
    /// Lobbies to leave on disconnect.
    lobbies: Mutex<HashSet<u64>>,
    push_tx: mpsc::UnboundedSender<Vec<u8>>,
}

async fn send<W: AsyncWrite + Unpin>(writer: &SharedWriter<W>, kind: u8, payload: &[u8]) {
    if let Err(e) = proto::send_frame(writer, kind, payload).await {
        tracing::debug!("steamworks: write failed: {e}");
    }
}

/// Serve one game after its hello.
pub async fn serve<R, W, B>(
    mut reader: R,
    writer: SharedWriter<W>,
    hello: &[u8],
    peer_pid: Option<u32>,
    backend: Arc<B>,
    router: Arc<Router>,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
    B: Backend,
{
    let refuse = |msg: String| welcome_payload(welcome::REFUSED, 0, &msg);
    let hello = match Hello::parse(hello) {
        Some(h) if h.version != PROTOCOL_VERSION => {
            let msg = format!("protocol {} unsupported", h.version);
            send(&writer, proto::C_SW_WELCOME, &refuse(msg)).await;
            return Ok(());
        }
        Some(h) if h.app_id == 0 => {
            let msg = "no app id".to_string();
            send(&writer, proto::C_SW_WELCOME, &refuse(msg)).await;
            return Ok(());
        }
        Some(h) => h,
        None => {
            let msg = "malformed hello".to_string();
            send(&writer, proto::C_SW_WELCOME, &refuse(msg)).await;
            return Ok(());
        }
    };
    let (steam_id, offline) = match backend.steam_id().await {
        Some(id) => (id, None),
        None => match offline_admission(&*backend, hello.app_id).await {
            Some(id) => (id, Some(id)),
            None => {
                let payload = welcome_payload(welcome::NO_SESSION, 0, "not logged in");
                send(&writer, proto::C_SW_WELCOME, &payload).await;
                return Ok(());
            }
        },
    };
    let pid = peer_pid.or((hello.pid != 0).then_some(hello.pid));
    // Offline: nothing reported to Steam.
    let launch = if offline.is_some() {
        None
    } else {
        match backend.start(hello.app_id, pid).await {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!(app_id = hello.app_id, "steamworks: launch refused: {e:#}");
                send(&writer, proto::C_SW_WELCOME, &refuse(format!("{e:#}"))).await;
                return Ok(());
            }
        }
    };
    let note = if offline.is_some() {
        tracing::info!(
            app_id = hello.app_id,
            ?pid,
            "steamworks: game connected offline (cached ownership only)"
        );
        OFFLINE_WELCOME
    } else {
        tracing::info!(app_id = hello.app_id, ?pid, "steamworks: game connected");
        ""
    };
    let payload = welcome_payload(welcome::OK, steam_id, note);
    send(&writer, proto::C_SW_WELCOME, &payload).await;

    let (push_tx, mut push_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    static NEXT_CONN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let conn_id = NEXT_CONN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Offline games never get purchases.
    if offline.is_none() {
        router.register_game(conn_id, hello.app_id, push_tx.clone());
    }
    let conn = Arc::new(Conn {
        backend,
        router,
        app_id: hello.app_id,
        offline,
        owned: Mutex::new(HashSet::new()),
        lobbies: Mutex::new(HashSet::new()),
        push_tx,
    });
    let lobby_pump = match offline {
        Some(_) => None,
        None => conn.backend.lobby_events(hello.app_id).await.map(|mut rx| {
            let tx = conn.push_tx.clone();
            tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    if tx.send(lobby::event_payload(&event)).is_err() {
                        break;
                    }
                }
            })
        }),
    };
    let persona_pump = match offline {
        Some(_) => None,
        None => conn.backend.persona_events().await.map(|mut rx| {
            let tx = conn.push_tx.clone();
            tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    for payload in persona_payloads(&event) {
                        if tx.send(payload).is_err() {
                            return;
                        }
                    }
                }
            })
        }),
    };
    let push_writer = writer.clone();
    let pusher = tokio::spawn(async move {
        while let Some(payload) = push_rx.recv().await {
            send(&push_writer, proto::C_SW_PUSH, &payload).await;
        }
    });

    let (frame_tx, mut frame_rx) = mpsc::unbounded_channel();
    let frames = tokio::spawn(async move {
        while let Ok(Some(frame)) = proto::read_frame(&mut reader).await {
            if frame_tx.send(frame).is_err() {
                break;
            }
        }
    });

    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            frame = frame_rx.recv() => match frame {
                Some((proto::C_SW_REQUEST, payload)) => {
                    let conn = Arc::clone(&conn);
                    let writer = writer.clone();
                    tasks.spawn(async move {
                        let reply = respond(&conn, &payload).await;
                        send(&writer, proto::C_SW_RESPONSE, &reply).await;
                    });
                }
                Some(_) => {}
                None => break,
            },
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }

    // Disconnected: drop in-flight work.
    tasks.shutdown().await;
    frames.abort();
    pusher.abort();
    if let Some(pump) = lobby_pump {
        pump.abort();
    }
    if let Some(pump) = persona_pump {
        pump.abort();
    }
    let joined: Vec<u64> = conn.lobbies.lock().unwrap().drain().collect();
    for lobby in joined {
        if let Err(e) = conn.backend.leave_lobby(hello.app_id, lobby).await {
            tracing::warn!(lobby, "steamworks: lobby leave failed: {e:#}");
        }
    }
    conn.router.unregister_game(conn_id);
    let owned: Vec<u32> = conn.owned.lock().unwrap().drain().collect();
    for handle in owned {
        conn.router.unregister(handle);
        if let Err(e) = conn.backend.cancel_auth_ticket(handle).await {
            tracing::warn!(handle, "steamworks: ticket cancel failed: {e:#}");
        }
    }
    if let Some(launch) = launch {
        conn.backend.stop(launch).await;
    }
    tracing::info!(app_id = hello.app_id, "steamworks: game disconnected");
    Ok(())
}

/// Welcome note for offline sessions.
pub const OFFLINE_WELCOME: &str = "offline: cached ownership only";

/// Offline account with a cached ticket.
async fn offline_admission<B: Backend>(backend: &B, app_id: u32) -> Option<u64> {
    let steam_id = backend.offline_steam_id().await?;
    match backend.cached_ownership_ticket(steam_id, app_id).await {
        Ok(_) => Some(steam_id),
        Err(e) => {
            tracing::info!(app_id, "steamworks: offline game refused: {e:#}");
            None
        }
    }
}

/// Answer one request payload.
async fn respond<B: Backend>(conn: &Conn<B>, payload: &[u8]) -> Vec<u8> {
    let Some(req) = Request::parse(payload) else {
        let id = Fields::new(payload).u32().unwrap_or(0);
        return response_payload(id, status::MALFORMED, b"malformed request");
    };
    let (code, body) = answer(conn, &req).await;
    response_payload(req.id, code, &body)
}

fn failed(e: &anyhow::Error) -> (u8, Vec<u8>) {
    (status::FAILED, format!("{e:#}").into_bytes())
}

async fn answer<B: Backend>(conn: &Conn<B>, req: &Request) -> (u8, Vec<u8>) {
    let app_id = if req.app_id == 0 {
        conn.app_id
    } else {
        req.app_id
    };
    let steam_id = conn.backend.steam_id().await;
    if req.command == cmd::STATUS {
        let mut body = vec![u8::from(steam_id.is_some())];
        body.extend_from_slice(&steam_id.unwrap_or(0).to_le_bytes());
        return (status::OK, body);
    }
    if lobby::cmd::is_lobby(req.command) {
        if conn.offline.is_some() || steam_id.is_none() {
            return (status::NO_SESSION, b"not logged in".to_vec());
        }
        return answer_lobby(conn, app_id, req.command, &req.body).await;
    }
    if !matches!(
        req.command,
        cmd::OWNERSHIP_TICKET
            | cmd::CREATE_AUTH_TICKET
            | cmd::CANCEL_AUTH_TICKET
            | cmd::ENCRYPTED_APP_TICKET
            | cmd::DLC_LIST
            | cmd::APP_INFO
            | cmd::SESSION_INFO
            | cmd::CLOUD_INFO
            | cmd::USER_STATS
            | cmd::STORE_STATS
            | cmd::FRIENDS
            | cmd::SET_RICH_PRESENCE
            | cmd::FRIEND_RICH_PRESENCE
            | cmd::OPEN_URL
            | cmd::GAME_INVITE
            | cmd::LOBBY_INVITE
            | cmd::UGC_QUERY
            | cmd::UGC_SUBSCRIBE
            | cmd::UGC_UNSUBSCRIBE
            | cmd::UGC_SUBSCRIBED
            | cmd::UGC_ITEM_STATE
            | cmd::UGC_DOWNLOAD
            | cmd::UGC_CREATE
            | cmd::UGC_UPDATE
            | cmd::AVATAR
    ) {
        return (status::UNSUPPORTED, b"unknown command".to_vec());
    }
    if let Some(offline_id) = conn.offline {
        return match req.command {
            cmd::OWNERSHIP_TICKET => {
                match conn
                    .backend
                    .cached_ownership_ticket(offline_id, app_id)
                    .await
                {
                    // Offline: owner unknown.
                    Ok(ticket) if wants_owner(req) => (status::OK, ticket_with_owner(ticket, None)),
                    Ok(ticket) => (status::OK, ticket),
                    Err(e) => failed(&e),
                }
            }
            cmd::APP_INFO => match conn.backend.offline_app_info(app_id).await {
                Ok(info) => (status::OK, app_info_body(&info)),
                Err(e) => failed(&e),
            },
            cmd::DLC_LIST => match conn.backend.offline_dlcs(app_id).await {
                Ok(Some(dlcs)) => (status::OK, dlc_list_body(&dlcs)),
                Ok(None) => (status::NO_SESSION, b"offline, no saved DLC list".to_vec()),
                Err(e) => failed(&e),
            },
            _ => (status::NO_SESSION, b"offline".to_vec()),
        };
    }
    let Some(steam_id) = steam_id else {
        return (status::NO_SESSION, b"not logged in".to_vec());
    };
    let mut f = Fields::new(&req.body);
    match req.command {
        cmd::OWNERSHIP_TICKET => match conn.backend.ownership_ticket(app_id).await {
            Ok(ticket) if wants_owner(req) => {
                let owner = match conn.backend.app_owner(app_id).await {
                    Ok(owner) => owner,
                    Err(e) => {
                        tracing::debug!(app_id, "steamworks: owner unknown: {e:#}");
                        None
                    }
                };
                (status::OK, ticket_with_owner(ticket, owner))
            }
            Ok(ticket) => (status::OK, ticket),
            Err(e) => failed(&e),
        },
        cmd::ENCRYPTED_APP_TICKET => {
            let userdata = f.rest().to_vec();
            match conn.backend.encrypted_app_ticket(app_id, userdata).await {
                Ok(outcome) => (status::OK, encrypted_ticket_body(&outcome)),
                Err(e) => failed(&e),
            }
        }
        cmd::DLC_LIST => match conn.backend.dlcs(app_id).await {
            Ok(dlcs) => (status::OK, dlc_list_body(&dlcs)),
            Err(e) => failed(&e),
        },
        cmd::APP_INFO => match conn.backend.app_info(app_id).await {
            Ok(info) => (status::OK, app_info_body(&info)),
            Err(e) => failed(&e),
        },
        cmd::SESSION_INFO => match conn.backend.session_info().await {
            Ok((time, country)) => (status::OK, session_info_body(time, &country)),
            Err(e) => failed(&e),
        },
        cmd::CLOUD_INFO => match conn.backend.cloud_info(steam_id, app_id).await {
            Ok(info) => (status::OK, cloud_info_body(&info)),
            Err(e) => failed(&e),
        },
        cmd::UGC_QUERY => {
            let Some(query) = UgcQuery::parse(&req.body) else {
                return (status::MALFORMED, b"bad query".to_vec());
            };
            match conn.backend.ugc_query(app_id, query).await {
                Ok(page) => (status::OK, ugc_page_body(&page)),
                Err(e) => failed(&e),
            }
        }
        cmd::UGC_SUBSCRIBE | cmd::UGC_UNSUBSCRIBE => {
            let Some(id) = f.u64() else {
                return (status::MALFORMED, b"no item".to_vec());
            };
            let on = req.command == cmd::UGC_SUBSCRIBE;
            match conn.backend.ugc_subscribe(app_id, id, on).await {
                Ok(()) => (status::OK, Vec::new()),
                Err(e) => failed(&e),
            }
        }
        cmd::UGC_SUBSCRIBED => match conn.backend.ugc_subscribed(app_id).await {
            Ok(ids) => {
                let count = u32::try_from(ids.len()).unwrap_or(u32::MAX);
                let mut body = count.to_le_bytes().to_vec();
                for id in ids.iter().take(count as usize) {
                    body.extend_from_slice(&id.to_le_bytes());
                }
                (status::OK, body)
            }
            Err(e) => failed(&e),
        },
        cmd::UGC_ITEM_STATE => {
            let Some(id) = f.u64() else {
                return (status::MALFORMED, b"no item".to_vec());
            };
            match conn.backend.ugc_state(app_id, id).await {
                Ok(state) => (status::OK, ugc_state_body(&state, ugc_progress(app_id, id))),
                Err(e) => failed(&e),
            }
        }
        cmd::UGC_DOWNLOAD => {
            let Some(id) = f.u64() else {
                return (status::MALFORMED, b"no item".to_vec());
            };
            start_ugc_download(conn, app_id, id);
            (status::OK, Vec::new())
        }
        cmd::UGC_CREATE | cmd::UGC_UPDATE if !conn.backend.workshop_publishing().await => (
            status::FAILED,
            b"Workshop publishing is off (aurelia config steamworks-client --workshop-publish on)"
                .to_vec(),
        ),
        cmd::UGC_CREATE => {
            let Some(file_type) = f.u32() else {
                return (status::MALFORMED, b"no file type".to_vec());
            };
            // Always the connected game's.
            match conn.backend.ugc_create(conn.app_id, file_type).await {
                Ok(id) => (status::OK, id.to_le_bytes().to_vec()),
                Err(e) => failed(&e),
            }
        }
        cmd::UGC_UPDATE => {
            let Some(update) = parse_ugc_update(&req.body) else {
                return (status::MALFORMED, b"bad item update".to_vec());
            };
            match conn.backend.ugc_update(conn.app_id, update).await {
                Ok(()) => (status::OK, Vec::new()),
                Err(e) => failed(&e),
            }
        }
        cmd::USER_STATS => {
            let Some(target) = f.u64() else {
                return (status::MALFORMED, b"no steam id".to_vec());
            };
            let target = (target != 0).then_some(target);
            match conn.backend.user_stats(app_id, target).await {
                Ok(stats) => {
                    let writes = target.is_none() && conn.backend.stats_writes().await;
                    (status::OK, user_stats_body(&stats, writes))
                }
                Err(e) => failed(&e),
            }
        }
        cmd::STORE_STATS => {
            if !conn.backend.stats_writes().await {
                return (status::FAILED, b"stats writes are off".to_vec());
            }
            let Some((crc, reset, changes)) = parse_store_request(&req.body) else {
                return (status::MALFORMED, b"bad store request".to_vec());
            };
            match conn.backend.store_stats(app_id, crc, reset, changes).await {
                Ok(outcome) => (status::OK, store_outcome_body(&outcome)),
                Err(e) => failed(&e),
            }
        }
        cmd::FRIENDS => match conn.backend.friends().await {
            Ok(snapshot) => (status::OK, friends_body(&snapshot)),
            Err(e) => failed(&e),
        },
        cmd::SET_RICH_PRESENCE => {
            let Some(pairs) = parse_pairs(&mut f) else {
                return (status::MALFORMED, b"bad rich presence".to_vec());
            };
            // Always the connected game's.
            match conn.backend.set_rich_presence(conn.app_id, pairs).await {
                Ok(()) => (status::OK, Vec::new()),
                Err(e) => failed(&e),
            }
        }
        cmd::FRIEND_RICH_PRESENCE => {
            let Some(id) = f.u64() else {
                return (status::MALFORMED, b"no steam id".to_vec());
            };
            match conn.backend.friend_rich_presence(id).await {
                Ok(pairs) => (status::OK, pairs_body(&pairs)),
                Err(e) => failed(&e),
            }
        }
        cmd::AVATAR => {
            let (Some(id), Some(size)) = (f.u64(), f.u8().and_then(AvatarSize::from_wire)) else {
                return (status::MALFORMED, b"bad avatar request".to_vec());
            };
            match conn.backend.avatar(id, size).await {
                Ok(avatar) => (status::OK, avatar_body(&avatar)),
                Err(e) => failed(&e),
            }
        }
        cmd::GAME_INVITE => {
            let (Some(friend), Some(connect)) = (f.u64(), f.blob()) else {
                return (status::MALFORMED, b"bad invite".to_vec());
            };
            let connect = String::from_utf8_lossy(connect).into_owned();
            match conn.backend.game_invite(app_id, friend, connect).await {
                Ok(()) => (status::OK, Vec::new()),
                Err(e) => failed(&e),
            }
        }
        cmd::LOBBY_INVITE => {
            let (Some(lobby), Some(friend)) = (f.u64(), f.u64()) else {
                return (status::MALFORMED, b"bad invite".to_vec());
            };
            match conn.backend.lobby_invite(app_id, lobby, friend).await {
                Ok(()) => (status::OK, Vec::new()),
                Err(e) => failed(&e),
            }
        }
        cmd::OPEN_URL => {
            let url = f.blob().map(String::from_utf8_lossy);
            let Some(url) = url.filter(|u| openable_url(u)) else {
                return (status::MALFORMED, b"not a web page".to_vec());
            };
            match conn.backend.open_url(url.into_owned()).await {
                Ok(opened) => (status::OK, vec![u8::from(opened)]),
                Err(e) => failed(&e),
            }
        }
        cmd::CREATE_AUTH_TICKET => {
            let kind = match (f.u8(), f.blob()) {
                (Some(ticket_kind::SESSION), Some(_)) => TicketKind::Session,
                (Some(ticket_kind::WEB_API), Some(id)) => {
                    let id = String::from_utf8_lossy(id).into_owned();
                    TicketKind::WebApi((!id.is_empty()).then_some(id))
                }
                _ => return (status::MALFORMED, b"bad ticket kind".to_vec()),
            };
            let pipe = u32::try_from(req.pipe).unwrap_or(0);
            match conn.backend.create_auth_ticket(app_id, pipe, kind).await {
                Ok(ticket) => {
                    conn.owned.lock().unwrap().insert(ticket.handle);
                    conn.router.register(ticket.handle, conn.push_tx.clone());
                    (status::OK, auth_ticket_body(&ticket))
                }
                Err(e) => failed(&e),
            }
        }
        _ => {
            let Some(handle) = f.u32() else {
                return (status::MALFORMED, b"no handle".to_vec());
            };
            // Only this game's tickets.
            if !conn.owned.lock().unwrap().remove(&handle) {
                return (status::OK, vec![0]);
            }
            conn.router.unregister(handle);
            match conn.backend.cancel_auth_ticket(handle).await {
                Ok(found) => (status::OK, vec![u8::from(found)]),
                Err(e) => failed(&e),
            }
        }
    }
}

/// Lobby command answers.
async fn answer_lobby<B: Backend>(
    conn: &Conn<B>,
    app_id: u32,
    command: u8,
    body: &[u8],
) -> (u8, Vec<u8>) {
    let malformed = || (status::MALFORMED, b"malformed lobby request".to_vec());
    let b = &conn.backend;
    let result = match command {
        lobby::cmd::CREATE => {
            let Some((ty, max)) = lobby::parse_create(body) else {
                return malformed();
            };
            b.create_lobby(app_id, ty, max).await.map(|(eresult, id)| {
                if eresult == 1 {
                    conn.lobbies.lock().unwrap().insert(id);
                }
                lobby::created_body(eresult, id)
            })
        }
        lobby::cmd::JOIN => {
            let Some(id) = lobby::parse_lobby(body) else {
                return malformed();
            };
            b.join_lobby(app_id, id).await.map(|j| {
                // Enter response 1 is success.
                if j.enter_response == 1 {
                    conn.lobbies.lock().unwrap().insert(id);
                }
                lobby::joined_body(j.enter_response, &j.state)
            })
        }
        lobby::cmd::LIST => {
            let Some((count, filters)) = lobby::parse_list(body) else {
                return malformed();
            };
            b.lobby_list(app_id, count, filters)
                .await
                .map(|(eresult, list)| lobby::list_body(eresult, &list))
        }
        lobby::cmd::LEAVE => {
            let Some(id) = lobby::parse_lobby(body) else {
                return malformed();
            };
            conn.lobbies.lock().unwrap().remove(&id);
            b.leave_lobby(app_id, id)
                .await
                .map(|eresult| eresult.to_le_bytes().to_vec())
        }
        lobby::cmd::SET_DATA => {
            let Some(update) = lobby::parse_update(body) else {
                return malformed();
            };
            b.set_lobby_data(app_id, update).await.map(|()| Vec::new())
        }
        lobby::cmd::CHAT => {
            let Some((id, message)) = lobby::parse_chat(body) else {
                return malformed();
            };
            b.send_lobby_chat(app_id, id, message)
                .await
                .map(|()| Vec::new())
        }
        _ => {
            let Some(id) = lobby::parse_lobby(body) else {
                return malformed();
            };
            b.request_lobby_data(app_id, id).await.map(|()| Vec::new())
        }
    };
    match result {
        Ok(body) => (status::OK, body),
        Err(e) => failed(&e),
    }
}

/// Starts once; pushes the result.
fn start_ugc_download<B: Backend>(conn: &Conn<B>, app_id: u32, id: u64) {
    let progress = {
        let mut downloads = UGC_DOWNLOADS.lock().unwrap();
        if downloads.contains_key(&(app_id, id)) {
            return;
        }
        let progress = Arc::new(std::sync::RwLock::new(DownloadState::default()));
        downloads.insert((app_id, id), Arc::clone(&progress));
        progress
    };
    let backend = Arc::clone(&conn.backend);
    let push_tx = conn.push_tx.clone();
    tokio::spawn(async move {
        let outcome = backend.ugc_download(app_id, id, progress).await;
        UGC_DOWNLOADS.lock().unwrap().remove(&(app_id, id));
        let result = match outcome {
            Ok(()) => ERESULT_OK,
            Err(e) => {
                tracing::warn!(app_id, id, "steamworks: Workshop download failed: {e:#}");
                ERESULT_FAIL
            }
        };
        let _ = push_tx.send(ugc_downloaded_payload(app_id, id, result));
    });
}

/// Old clients send no flags.
fn wants_owner(req: &Request) -> bool {
    req.body
        .first()
        .is_some_and(|f| f & ownership_flag::WITH_OWNER != 0)
}

/// The daemon's real backend.
pub struct DaemonBackend;

/// Logged-on shared client, if any.
async fn authed_client() -> Option<SteamClient> {
    let client = super::shared_restored_client().await;
    client.is_authenticated().then_some(client)
}

fn not_logged_in() -> anyhow::Error {
    anyhow!("not logged in")
}

impl Backend for DaemonBackend {
    async fn steam_id(&self) -> Option<u64> {
        authed_client().await?.steam_id()
    }

    async fn start(&self, app_id: u32, pid: Option<u32>) -> Result<u64> {
        playing::start_launch(&GAMES, Launch { app_id, pid }, authed_client).await
    }

    async fn stop(&self, launch: u64) {
        if let Err(e) = playing::stop_launch(&GAMES, launch, &authed_client).await {
            tracing::warn!("steamworks: could not clear games played: {e:#}");
        }
    }

    async fn app_owner(&self, app_id: u32) -> Result<Option<u64>> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.app_owner(app_id).await
    }

    async fn ownership_ticket(&self, app_id: u32) -> Result<Vec<u8>> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.ownership_ticket(app_id).await
    }

    async fn offline_steam_id(&self) -> Option<u64> {
        if authed_client().await.is_some() {
            return None;
        }
        aurelia::core::config::load_session().await.ok()?.steam_id
    }

    async fn cached_ownership_ticket(&self, steam_id: u64, app_id: u32) -> Result<Vec<u8>> {
        use aurelia::steam_client::ticket_cache::{TicketCache, TicketSource, now_rtime};
        let source = TicketSource::Offline {
            steam_id: Some(steam_id),
        };
        tokio::task::spawn_blocking(move || {
            TicketCache::open()?.resolve(source, app_id, now_rtime())
        })
        .await?
    }

    async fn create_auth_ticket(
        &self,
        app_id: u32,
        pipe: u32,
        kind: TicketKind,
    ) -> Result<AuthSessionTicket> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.create_auth_ticket(app_id, pipe, kind).await
    }

    async fn cancel_auth_ticket(&self, handle: u32) -> Result<bool> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.cancel_auth_ticket(handle).await
    }

    async fn ticket_events(&self) -> Vec<TicketAuthEvent> {
        // No logon from the poller.
        match super::current_client().await {
            Some(client) => client.take_ticket_auth_events(),
            None => Vec::new(),
        }
    }

    async fn encrypted_app_ticket(
        &self,
        app_id: u32,
        userdata: Vec<u8>,
    ) -> Result<EncryptedTicketOutcome> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client
            .request_encrypted_app_ticket_with_data(app_id, &userdata)
            .await
    }

    async fn dlcs(&self, app_id: u32) -> Result<Vec<DlcInfo>> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        let language = game_language(&client, app_id).await;
        client.steamworks_dlcs(app_id, &language).await
    }

    async fn app_info(&self, app_id: u32) -> Result<AppInstallInfo> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        local_app_info(&client, app_id).await
    }

    async fn offline_app_info(&self, app_id: u32) -> Result<AppInstallInfo> {
        local_app_info(&SteamClient::new()?, app_id).await
    }

    async fn offline_dlcs(&self, app_id: u32) -> Result<Option<Vec<DlcInfo>>> {
        let dir = aurelia::steam_client::steamworks_info::dlc_catalog_dir()?;
        SteamClient::new()?
            .offline_steamworks_dlcs(app_id, &dir)
            .await
    }

    async fn create_lobby(&self, app_id: u32, lobby_type: i32, max: i32) -> Result<(i32, u64)> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.create_lobby(app_id, lobby_type, max).await
    }

    async fn join_lobby(&self, app_id: u32, lobby: u64) -> Result<JoinedLobby> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.join_lobby(app_id, lobby).await
    }

    async fn leave_lobby(&self, app_id: u32, lobby: u64) -> Result<i32> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.leave_lobby(app_id, lobby).await
    }

    async fn lobby_list(
        &self,
        app_id: u32,
        count: i32,
        filters: Vec<LobbyFilter>,
    ) -> Result<(i32, Vec<LobbyState>)> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.lobby_list(app_id, count, &filters).await
    }

    async fn set_lobby_data(&self, app_id: u32, update: LobbyUpdate) -> Result<()> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.set_lobby_data(app_id, &update).await
    }

    async fn send_lobby_chat(&self, app_id: u32, lobby: u64, message: Vec<u8>) -> Result<()> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.send_lobby_chat(app_id, lobby, message).await
    }

    async fn request_lobby_data(&self, app_id: u32, lobby: u64) -> Result<()> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.request_lobby_data(app_id, lobby).await
    }

    async fn lobby_events(&self, app_id: u32) -> Option<mpsc::UnboundedReceiver<LobbyEvent>> {
        authed_client().await?.lobby_events(app_id).ok()
    }

    async fn persona_events(&self) -> Option<mpsc::UnboundedReceiver<PersonaEvent>> {
        authed_client().await?.persona_events().ok()
    }

    async fn friends(&self) -> Result<FriendsSnapshot> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        let me = client.steam_id().ok_or_else(not_logged_in)?;
        let roster = super::shared_full_roster().await;
        let own = roster.iter().find(|f| f.steam_id == me);
        let configured = aurelia::core::config::load_launcher_config()
            .await
            .map(|c| c.chat_presence.persona_state())
            .unwrap_or(1);
        Ok(FriendsSnapshot {
            name: own.and_then(|f| f.persona_name.clone()).unwrap_or_default(),
            state: own.and_then(|f| f.persona_state).unwrap_or(configured),
            friends: roster
                .into_iter()
                .filter(|f| f.steam_id != me && f.relationship != 0)
                .collect(),
        })
    }

    async fn set_rich_presence(&self, app_id: u32, pairs: Vec<(String, String)>) -> Result<()> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.upload_rich_presence(app_id, &pairs).await
    }

    async fn friend_rich_presence(&self, steam_id: u64) -> Result<Vec<(String, String)>> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.friend_rich_presence(steam_id).await
    }

    async fn game_invite(&self, app_id: u32, friend: u64, connect: String) -> Result<()> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        let connect = (!connect.is_empty()).then_some(connect);
        client.invite_to_game(friend, app_id, connect).await
    }

    async fn lobby_invite(&self, app_id: u32, lobby: u64, friend: u64) -> Result<()> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.invite_to_lobby(app_id, lobby, friend).await
    }

    async fn open_url(&self, url: String) -> Result<bool> {
        Ok(tokio::task::spawn_blocking(move || spawn_browser(&url)).await?)
    }

    async fn avatar(&self, steam_id: u64, size: AvatarSize) -> Result<Arc<Avatar>> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        let hash = || async {
            super::shared_full_roster()
                .await
                .into_iter()
                .find(|f| f.steam_id == steam_id)
                .and_then(|f| f.avatar_hash)
        };
        let mut hex = hash().await;
        if hex.is_none() {
            // Ask Steam, wait briefly.
            client.request_friend_data(&[steam_id]).await?;
            let deadline = tokio::time::Instant::now() + AVATAR_WAIT;
            while hex.is_none() && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(200)).await;
                hex = hash().await;
            }
        }
        let hex = hex.ok_or_else(|| anyhow!("Steam sent no avatar for {steam_id}"))?;
        fetch_avatar(&hex, size).await
    }

    async fn session_info(&self) -> Result<(u32, String)> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        let now = match client.server_time().await {
            Ok(now) => now,
            Err(e) => {
                // Local clock until Steam answers.
                tracing::debug!("steamworks: {e:#}; using local time");
                aurelia::steam_client::server_time::apply(
                    aurelia::steam_client::server_time::local_ms(),
                    0,
                )
            }
        };
        Ok((now, client.ip_country().unwrap_or_default()))
    }

    async fn cloud_info(&self, steam_id: u64, app_id: u32) -> Result<RemoteStorageInfo> {
        let config = aurelia::core::config::LauncherConfig::load().await?;
        remote_storage_info(&config, steam_id, app_id)
    }

    async fn ugc_query(&self, app_id: u32, query: UgcQuery) -> Result<UgcPage> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        match query {
            UgcQuery::All {
                query_type,
                page,
                cursor,
                search,
                tags,
                excluded,
            } => {
                let per_page = UGC_PAGE_SIZE as u32;
                client
                    .ugc_query(
                        app_id,
                        query_type,
                        page,
                        cursor.as_deref(),
                        Some(&search),
                        &tags,
                        &excluded,
                        per_page,
                    )
                    .await
            }
            UgcQuery::User {
                account,
                list,
                page,
                tags,
                excluded,
            } => {
                if list != UGC_LIST_PUBLISHED {
                    return Err(anyhow!("user list {list} is not supported"));
                }
                let per_page = UGC_PAGE_SIZE as u32;
                client
                    .ugc_published_by(app_id, account, page, &tags, &excluded, per_page)
                    .await
            }
            UgcQuery::Details(ids) => {
                let items = client.ugc_details(&ids).await?;
                Ok(UgcPage {
                    total: u32::try_from(items.len()).unwrap_or(u32::MAX),
                    next_cursor: String::new(),
                    items,
                })
            }
            UgcQuery::Subscribed { page } => {
                let ids = client.fetch_subscribed_items(app_id).await?;
                let start = (page.max(1) as usize - 1).saturating_mul(UGC_PAGE_SIZE);
                let slice: Vec<u64> = ids
                    .iter()
                    .skip(start)
                    .take(UGC_PAGE_SIZE)
                    .copied()
                    .collect();
                Ok(UgcPage {
                    total: u32::try_from(ids.len()).unwrap_or(u32::MAX),
                    next_cursor: String::new(),
                    items: client.ugc_details(&slice).await?,
                })
            }
        }
    }

    async fn ugc_subscribe(&self, app_id: u32, id: u64, subscribe: bool) -> Result<()> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        if subscribe {
            client.subscribe_published_file(id, app_id).await
        } else {
            client.unsubscribe_published_file(id, app_id).await
        }
    }

    async fn ugc_subscribed(&self, app_id: u32) -> Result<Vec<u64>> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.fetch_subscribed_items(app_id).await
    }

    async fn workshop_publishing(&self) -> bool {
        aurelia::core::config::load_launcher_config()
            .await
            .is_ok_and(|c| c.steamworks_workshop_publish)
    }

    async fn ugc_create(&self, app_id: u32, file_type: u32) -> Result<u64> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        tracing::info!(
            app_id,
            file_type,
            "steamworks: game creates a Workshop item"
        );
        client.ugc_create_item(app_id, file_type).await
    }

    async fn ugc_update(&self, app_id: u32, update: UgcUpdate) -> Result<()> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        tracing::info!(
            app_id,
            id = update.id,
            "steamworks: game updates a Workshop item"
        );
        client.ugc_update_item(app_id, &update).await
    }

    async fn ugc_state(&self, app_id: u32, id: u64) -> Result<UgcInstall> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.ugc_install_state(app_id, id).await
    }

    async fn ugc_download(
        &self,
        app_id: u32,
        id: u64,
        progress: Arc<std::sync::RwLock<DownloadState>>,
    ) -> Result<()> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        client.ugc_download(app_id, id, progress).await
    }

    async fn user_stats(&self, app_id: u32, steam_id: Option<u64>) -> Result<UserStats> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        let language = game_language(&client, app_id).await;
        client.user_stats(app_id, steam_id, &language).await
    }

    async fn stats_writes(&self) -> bool {
        aurelia::core::config::load_launcher_config()
            .await
            .is_ok_and(|c| c.steamworks_stats_writes)
    }

    async fn store_stats(
        &self,
        app_id: u32,
        crc: u32,
        reset: bool,
        changes: Vec<(u32, u32)>,
    ) -> Result<StoreOutcome> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        tracing::info!(
            app_id,
            stats = changes.len(),
            reset,
            "steamworks: storing stats"
        );
        client.store_user_stats(app_id, crc, reset, &changes).await
    }
}

/// Launch a browser, else log.
fn spawn_browser(url: &str) -> bool {
    let has_display =
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if has_display || cfg!(target_os = "macos") {
        let spawned = std::process::Command::new(opener)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Ok(mut child) = spawned {
            // Reap it off-thread.
            std::thread::spawn(move || child.wait());
            tracing::info!("steamworks: opened {url} for a game");
            return true;
        }
    }
    tracing::warn!("steamworks: a game wants {url} shown; open it in a browser");
    false
}

/// Appmanifest facts, configured language fallback.
async fn local_app_info(client: &SteamClient, app_id: u32) -> Result<AppInstallInfo> {
    let mut info = client.app_install_info(app_id).await?;
    if info.language.is_none() {
        info.language = Some(crate::commands::common::resolve_steam_language(None).await);
    }
    Ok(info)
}

/// Manifest language, else configured.
async fn game_language(client: &SteamClient, app_id: u32) -> String {
    match client.app_install_info(app_id).await {
        Ok(AppInstallInfo {
            language: Some(lang),
            ..
        }) => lang,
        _ => crate::commands::common::resolve_steam_language(None).await,
    }
}

#[cfg(test)]
#[path = "steamworks_tests.rs"]
mod tests;
