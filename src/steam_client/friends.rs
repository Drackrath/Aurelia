//! Friends roster: maintain the logged-in user's friends list and their live
//! persona state (online status, current game) over the shared Steam connection.
//!
//! Steam pushes two relevant messages: `CMsgClientFriendsList` (who your friends
//! are and the relationship to each) and `CMsgClientPersonaState` (per-friend
//! display name, online status, and current game). This module folds both into a
//! [`Roster`] map and exposes a long-lived watcher plus a one-shot snapshot
//! helper. As with the other `SteamClient` submodules, the struct and shared
//! imports live in the parent module and are pulled in via `use super::*`.
use super::*;
use regex::Regex;
use std::sync::{LazyLock, RwLock};
use steam_vent::NetMessage;
use steam_vent_proto::steammessages_clientserver_friends::{
    CMsgClientAddFriend, CMsgClientAddFriendResponse, CMsgClientChangeStatus, CMsgClientFriendsList,
    CMsgClientPersonaState, CMsgClientRemoveFriend, CMsgClientRequestFriendData,
};
use tokio_stream::StreamExt;

/// EFriendRelationship value for an accepted friend.
///
/// EFriendRelationship: 0=None, 1=Blocked, 2=RequestRecipient, 3=Friend,
/// 4=RequestInitiator, 5=Ignored, 6=IgnoredFriend.
pub const RELATIONSHIP_FRIEND: u32 = 3;

/// EClientPersonaStateFlag bits to request when asking Steam for friend data:
/// Status(1) | PlayerName(2) | Presence(16) | GameExtraInfo(256) |
/// GameDataBlob(512) = 787. These cover online status, display name, and the
/// currently-played game.
pub const PERSONA_REQUEST_FLAGS: u32 = 1 | 2 | 16 | 256 | 512;

/// Maximum friends per `CMsgClientRequestFriendData` request. Steam answers each
/// request with a burst of `CMsgClientPersonaState` messages, and steam-vent's
/// per-kind notification buffer holds only 16; requesting in small chunks (and
/// draining the responses between them) keeps a burst from overflowing that
/// buffer, which would otherwise drop persona updates as the stream reports
/// `Lagged`.
const PERSONA_REQUEST_CHUNK: usize = 8;

/// Idle gap that marks the end of a chunk's persona-response burst.
const PERSONA_DRAIN_IDLE: std::time::Duration = std::time::Duration::from_millis(500);

/// A single friend and the latest known information about them.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Friend {
    /// SteamID64 of the friend.
    pub steam_id: u64,
    /// Raw EFriendRelationship value (see [`RELATIONSHIP_FRIEND`]).
    pub relationship: u32,
    /// Display name, if a persona state update has supplied one.
    pub persona_name: Option<String>,
    /// Online status: 0 offline, 1 online, 2 busy, 3 away, 4 snooze,
    /// 5 looking-to-trade, 6 looking-to-play.
    pub persona_state: Option<u32>,
    /// App id of the game the friend is currently playing, or None if not in-game.
    pub game_app_id: Option<u32>,
    /// Name of the game the friend is currently playing, if known.
    pub game_name: Option<String>,
}

impl Friend {
    /// A bare entry with only its id and relationship set; persona fields are
    /// filled in later from `CMsgClientPersonaState`.
    fn new(steam_id: u64, relationship: u32) -> Self {
        Friend {
            steam_id,
            relationship,
            persona_name: None,
            persona_state: None,
            game_app_id: None,
            game_name: None,
        }
    }
}

/// The friends roster, keyed by SteamID64.
pub type Roster = HashMap<u64, Friend>;

/// A user resolved from a search query (see [`resolve_steam_id`]).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResolvedUser {
    /// SteamID64.
    pub steam_id: u64,
    /// Display (persona) name, if the profile exposed one.
    pub persona_name: Option<String>,
    /// Canonical Steam Community profile URL.
    pub profile_url: String,
}

/// The result of sending a friend request (see [`SteamClient::add_friend`]).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AddedFriend {
    /// SteamID64 the request was sent to.
    pub steam_id: u64,
    /// The target's display name, if Steam returned one.
    pub persona_name: Option<String>,
}

/// SteamID64 base for an individual account (`0x0110000100000000`). Any 64-bit id
/// at or above this is treated as a ready-to-use SteamID rather than a vanity name.
const STEAMID64_INDIVIDUAL_BASE: u64 = 76_561_197_960_265_728;

static RE_PROFILE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"steamcommunity\.com/profiles/(\d{17})").unwrap());
static RE_VANITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"steamcommunity\.com/id/([^/?#\s]+)").unwrap());
static RE_STEAMID64: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<steamID64>(\d+)</steamID64>").unwrap());
static RE_STEAM_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<steamID>(.*?)</steamID>").unwrap());

fn nonempty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Fetch a Steam Community profile XML document (the `?xml=1` view, which needs no
/// API key) with a short timeout.
async fn fetch_community_xml(url: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .user_agent("aurelia")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("failed to build HTTP client")?;
    client
        .get(url)
        .send()
        .await
        .context("request to Steam Community failed")?
        .text()
        .await
        .context("failed reading the Steam Community response")
}

/// Pull the persona name out of a profile XML (`<steamID>` element, possibly
/// CDATA-wrapped).
fn parse_community_name(xml: &str) -> Option<String> {
    let raw = RE_STEAM_NAME.captures(xml)?.get(1)?.as_str().trim();
    let inner = raw
        .strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .unwrap_or(raw);
    nonempty(inner)
}

/// Resolve a free-form `query` to a Steam account, **without** a Steam session.
///
/// Steam exposes no people-search over the CM connection, so this resolves an
/// *identifier* via the public Steam Community `?xml=1` endpoint. Accepts:
/// - a 17-digit **SteamID64** (returned as-is, name looked up best-effort),
/// - a **profile URL** (`steamcommunity.com/profiles/<id>`),
/// - a **custom/vanity URL** (`steamcommunity.com/id/<name>`), or
/// - a bare **vanity name** (the custom-URL slug).
pub async fn resolve_steam_id(query: &str) -> Result<ResolvedUser> {
    let q = query.trim();
    if q.is_empty() {
        bail!("empty search query");
    }

    // A profile URL, or a bare SteamID64.
    let direct_id = RE_PROFILE_ID
        .captures(q)
        .and_then(|c| c[1].parse::<u64>().ok())
        .or_else(|| match q.parse::<u64>() {
            Ok(v) if v >= STEAMID64_INDIVIDUAL_BASE => Some(v),
            _ => None,
        });
    if let Some(steam_id) = direct_id {
        let xml = fetch_community_xml(&format!(
            "https://steamcommunity.com/profiles/{steam_id}/?xml=1"
        ))
        .await
        .unwrap_or_default();
        return Ok(ResolvedUser {
            steam_id,
            persona_name: parse_community_name(&xml),
            profile_url: format!("https://steamcommunity.com/profiles/{steam_id}"),
        });
    }

    // Otherwise treat it as a vanity (custom-URL) name, whether a full URL or bare slug.
    let slug = RE_VANITY
        .captures(q)
        .map(|c| c[1].to_string())
        .unwrap_or_else(|| q.trim_matches('/').to_string());
    let xml = fetch_community_xml(&format!("https://steamcommunity.com/id/{slug}/?xml=1")).await?;
    let steam_id = RE_STEAMID64
        .captures(&xml)
        .and_then(|c| c[1].parse::<u64>().ok())
        .ok_or_else(|| {
            anyhow!("could not resolve '{slug}' to a Steam account (no such profile or custom URL)")
        })?;
    Ok(ResolvedUser {
        steam_id,
        persona_name: parse_community_name(&xml),
        profile_url: format!("https://steamcommunity.com/id/{slug}"),
    })
}

/// A friendlier message for a non-OK `EResult` from a friend request.
fn add_friend_error(eresult: i32) -> String {
    match eresult {
        15 => "access denied — the account may have blocked you or restricts who can add it"
            .to_string(),
        25 => "your friends list is full (Steam friend limit reached)".to_string(),
        84 => "Steam is rate-limiting friend requests — wait a while and try again".to_string(),
        other => format!("Steam rejected the friend request (EResult {other})"),
    }
}

/// SteamID64s of every entry that is an accepted friend (the ones worth
/// requesting persona data for).
fn friend_ids(roster: &Roster) -> Vec<u64> {
    roster
        .values()
        .filter(|f| f.relationship == RELATIONSHIP_FRIEND)
        .map(|f| f.steam_id)
        .collect()
}

/// Fold a `CMsgClientFriendsList` into the roster.
///
/// A non-incremental message is a full snapshot, so the roster is cleared first.
/// Relationship 0 (None) means the friend was removed; any other relationship is
/// an upsert that preserves already-known persona fields.
pub fn apply_friends_list(roster: &mut Roster, msg: &CMsgClientFriendsList) {
    if !msg.bincremental() {
        roster.clear();
    }
    for f in &msg.friends {
        let id = f.ulfriendid();
        let rel = f.efriendrelationship();
        if rel == 0 {
            roster.remove(&id);
            continue;
        }
        match roster.get_mut(&id) {
            Some(existing) => existing.relationship = rel,
            None => {
                roster.insert(id, Friend::new(id, rel));
            }
        }
    }
}

/// Fold a `CMsgClientPersonaState` into the roster, updating each friend's
/// display name, online status, and current game.
///
/// An empty incoming name never clobbers a known one; an app id of 0 clears the
/// in-game fields.
pub fn apply_persona_state(roster: &mut Roster, msg: &CMsgClientPersonaState) {
    for fr in &msg.friends {
        let id = fr.friendid();
        let entry = roster.entry(id).or_insert_with(|| Friend::new(id, 0));

        let name = fr.player_name();
        if !name.is_empty() {
            entry.persona_name = Some(name.to_string());
        }
        entry.persona_state = Some(fr.persona_state());
        entry.game_app_id = match fr.game_played_app_id() {
            0 => None,
            app_id => Some(app_id),
        };
        let game_name = fr.game_name();
        entry.game_name = (!game_name.is_empty()).then(|| game_name.to_string());
    }
}

/// Drain steam-vent's buffer of messages that arrived with no registered
/// subscriber (its internal `rest` ring buffer) and fold any friends-list /
/// persona-state messages into `roster`.
///
/// Steam pushes the friends list exactly once, right after logon. A subscription
/// created later (`on::<T>()`) never sees it — the message has already been routed
/// to `rest`. Draining `rest` recovers it. Friends-list messages are applied
/// before persona-state ones so a full-snapshot list can't wipe persona data that
/// arrived in the same burst.
fn drain_unprocessed_into(connection: &Connection, roster: &mut Roster) {
    let mut lists = Vec::new();
    let mut personas = Vec::new();
    for raw in connection.take_unprocessed() {
        if raw.kind == <CMsgClientFriendsList as NetMessage>::KIND {
            if let Ok(msg) = raw.into_message::<CMsgClientFriendsList>() {
                lists.push(msg);
            }
        } else if raw.kind == <CMsgClientPersonaState as NetMessage>::KIND {
            if let Ok(msg) = raw.into_message::<CMsgClientPersonaState>() {
                personas.push(msg);
            }
        }
    }
    for list in &lists {
        apply_friends_list(roster, list);
    }
    for persona in &personas {
        apply_persona_state(roster, persona);
    }
}

impl SteamClient {
    /// Announce an online persona so Steam starts delivering friend persona data
    /// (display names, status, current game) and incoming chat. A refresh-token
    /// logon is "offline" by default, and Steam withholds friend persona state —
    /// and friend messages — until the client declares a persona. `persona_state`
    /// is a raw EPersonaState (1 = online, 7 = invisible); `need_persona_response`
    /// asks Steam to push the friends' persona state in reply.
    pub async fn announce_persona(&self, persona_state: u32) -> Result<()> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        let mut status = CMsgClientChangeStatus::new();
        status.set_persona_state(persona_state);
        status.set_need_persona_response(true);
        connection
            .send(status)
            .await
            .map_err(|e| anyhow!("failed to announce persona status: {e}"))?;
        Ok(())
    }

    /// Announce the presence configured in `LauncherConfig::chat_presence`
    /// (defaults to invisible/"offline"). Falls back to invisible if the config
    /// can't be read. Public so an interactive chat session can announce presence
    /// when running standalone (the daemon's friends watcher already does so).
    pub async fn announce_configured_presence(&self) -> Result<()> {
        let persona_state = crate::config::load_launcher_config()
            .await
            .map(|c| c.chat_presence.persona_state())
            .unwrap_or(7);
        self.announce_persona(persona_state).await
    }

    /// Ask Steam to push persona state for the given friends. Best-effort and
    /// fire-and-forget: the responses arrive asynchronously as
    /// `CMsgClientPersonaState` messages. A no-op for an empty id list.
    pub async fn request_friend_data(&self, ids: &[u64]) -> Result<()> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        if ids.is_empty() {
            return Ok(());
        }
        let mut req = CMsgClientRequestFriendData::new();
        req.set_persona_state_requested(PERSONA_REQUEST_FLAGS);
        req.friends = ids.to_vec();
        connection
            .send(req)
            .await
            .map_err(|e| anyhow!("failed to request friend data: {e}"))?;
        Ok(())
    }

    /// Maintain `roster` for the lifetime of the connection by folding in every
    /// friends-list and persona-state update Steam pushes, requesting fresh
    /// persona data whenever the friends list changes.
    ///
    /// This runs until the connection's streams end. The logon-time friends list
    /// may be missed if this subscription loses the race against the initial
    /// push, but it self-heals on the next friends-list update; persona updates
    /// arrive continuously regardless.
    pub async fn run_friends_watcher(&self, roster: Arc<RwLock<Roster>>) -> Result<()> {
        // Keep a local clone of the connection alive: the streams returned by
        // `on::<T>()` are tied to the connection value's lifetime.
        let connection = self
            .connection
            .as_ref()
            .cloned()
            .context("steam connection not initialized")?;
        let mut friends_stream = connection.on::<CMsgClientFriendsList>();
        let mut persona_stream = connection.on::<CMsgClientPersonaState>();

        // Announce the configured presence so Steam delivers friend persona data
        // (and incoming chat). A refresh-token session is otherwise offline.
        if let Err(e) = self.announce_configured_presence().await {
            tracing::warn!("failed to announce presence: {e}");
        }

        // Recover the logon-time friends/persona burst that landed in `rest` before
        // the subscriptions above existed; later updates arrive via the streams.
        let initial_ids: Vec<u64> = {
            let mut guard = roster.write().expect("roster lock poisoned");
            drain_unprocessed_into(&connection, &mut guard);
            friend_ids(&guard)
        };

        // Pull persona data (names/status/games) for the initial friends in small
        // chunks, draining each chunk's response burst before requesting the next so
        // it never overflows steam-vent's 16-slot notification buffer.
        for chunk in initial_ids.chunks(PERSONA_REQUEST_CHUNK) {
            if let Err(e) = self.request_friend_data(chunk).await {
                tracing::warn!("failed to request friend data: {e}");
                continue;
            }
            while let Ok(item) =
                tokio::time::timeout(PERSONA_DRAIN_IDLE, persona_stream.next()).await
            {
                match item {
                    Some(Ok(state)) => {
                        let mut guard = roster.write().expect("roster lock poisoned");
                        apply_persona_state(&mut guard, &state);
                    }
                    Some(Err(_)) => continue, // lagged — keep draining
                    None => break,            // stream closed
                }
            }
        }

        tracing::info!("friends watcher started ({} friend(s) so far)", initial_ids.len());
        loop {
            tokio::select! {
                Some(Ok(list)) = friends_stream.next() => {
                    let ids: Vec<u64> = {
                        let mut guard = roster.write().expect("roster lock poisoned");
                        apply_friends_list(&mut guard, &list);
                        friend_ids(&guard)
                    };
                    if let Err(e) = self.request_friend_data(&ids).await {
                        tracing::warn!("failed to request friend data: {e}");
                    }
                }
                Some(Ok(state)) = persona_stream.next() => {
                    let mut guard = roster.write().expect("roster lock poisoned");
                    apply_persona_state(&mut guard, &state);
                }
                else => break,
            }
        }
        tracing::info!("friends watcher stopped");
        Ok(())
    }

    /// Collect a one-shot, bounded snapshot of the friends roster for standalone
    /// (non-daemon) use, waiting up to `wait` for messages to arrive.
    ///
    /// Without the long-running daemon watcher this is best-effort and may return
    /// an empty or partial list, because the logon-time friends list has usually
    /// already been consumed by the time this subscribes. Returned friends are
    /// sorted by persona name (None last) then SteamID64.
    pub async fn collect_friends(&self, wait: std::time::Duration) -> Result<Vec<Friend>> {
        // Keep the connection clone alive for the duration of the streams.
        let connection = self
            .connection
            .as_ref()
            .cloned()
            .context("steam connection not initialized")?;
        // Only the persona stream is needed: the friends list itself is recovered
        // from `rest` below, not awaited live.
        let mut persona_stream = connection.on::<CMsgClientPersonaState>();

        // Announce the configured presence so Steam delivers friend persona data.
        let _ = self.announce_configured_presence().await;

        let mut roster: Roster = HashMap::new();
        let deadline = tokio::time::Instant::now() + wait;

        // Recover the logon-time friends list from `rest` (see
        // `drain_unprocessed_into`), then pull persona data for those friends in
        // small chunks, draining each chunk's response burst before the next so it
        // never overflows steam-vent's 16-slot notification buffer.
        drain_unprocessed_into(&connection, &mut roster);
        let ids = friend_ids(&roster);
        for chunk in ids.chunks(PERSONA_REQUEST_CHUNK) {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            if self.request_friend_data(chunk).await.is_err() {
                continue;
            }
            while let Ok(item) =
                tokio::time::timeout(PERSONA_DRAIN_IDLE, persona_stream.next()).await
            {
                match item {
                    Some(Ok(state)) => apply_persona_state(&mut roster, &state),
                    Some(Err(_)) => continue,
                    None => break,
                }
            }
        }

        let mut friends: Vec<Friend> = roster.into_values().collect();
        friends.sort_by(|a, b| match (&a.persona_name, &b.persona_name) {
            (Some(x), Some(y)) => x.cmp(y).then(a.steam_id.cmp(&b.steam_id)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.steam_id.cmp(&b.steam_id),
        });
        Ok(friends)
    }
}

impl SteamClient {
    /// Send a friend request to `steam_id`. Waits for Steam's response and reports
    /// the resolved account (and its display name) on success.
    pub async fn add_friend(&self, steam_id: u64) -> Result<AddedFriend> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        let mut req = CMsgClientAddFriend::new();
        req.set_steamid_to_add(steam_id);

        let resp: CMsgClientAddFriendResponse =
            tokio::time::timeout(std::time::Duration::from_secs(15), connection.job(req))
                .await
                .map_err(|_| anyhow!("timed out waiting for Steam to confirm the friend request"))?
                .map_err(|e| anyhow!("friend request failed: {e}"))?;

        if resp.eresult() != 1 {
            bail!("{}", add_friend_error(resp.eresult()));
        }
        let added_id = match resp.steam_id_added() {
            0 => steam_id,
            id => id,
        };
        Ok(AddedFriend {
            steam_id: added_id,
            persona_name: nonempty(resp.persona_name_added()),
        })
    }

    /// Remove a friend, or cancel/decline a pending request, by SteamID64.
    /// Fire-and-forget — Steam sends no acknowledgement.
    pub async fn remove_friend(&self, steam_id: u64) -> Result<()> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        let mut req = CMsgClientRemoveFriend::new();
        req.set_friendid(steam_id);
        connection
            .send(req)
            .await
            .map_err(|e| anyhow!("failed to remove friend: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
}
