//! Friend chat: send and receive direct messages over the shared Steam
//! connection, and fetch recent message history.
//!
//! The live send/receive path wraps the `ChatClient` from our
//! `aurelia-steam-vent-chat` fork; history uses the `FriendMessages`
//! `GetRecentMessages` service method directly. As with the other `SteamClient`
//! submodules, the struct and shared imports live in the parent module and are
//! pulled in via `use super::*`.
use super::*;
use steam_vent::ReceivableMessage;
use steam_vent_chat::ChatClient;
use steam_vent_proto::steammessages_clientserver::CMsgClientInviteToGame;
use steam_vent_proto::steammessages_clientserver_2::CMsgClientPlayingSessionState;
use steam_vent_proto::steammessages_friendmessages_steamclient::{
    CFriendMessages_GetRecentMessages_Request, CFriendMessages_GetRecentMessages_Response,
};
use steamid_ng::SteamID;
use tokio_stream::StreamExt;

/// Wait for Steam's playing-session state.
const PLAYING_STATE_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Account-wide game session Steam reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct PlayingSession {
    /// Another session holds the game.
    pub blocked: bool,
    /// App being played; 0 is none.
    pub playing_app: u32,
}

/// Last state: (session id, state).
static PLAYING: std::sync::Mutex<Option<(i32, PlayingSession)>> = std::sync::Mutex::new(None);

/// Store a pushed playing-session state.
pub(crate) fn record_playing_session(session_id: i32, msg: &CMsgClientPlayingSessionState) {
    let state = PlayingSession {
        blocked: msg.playing_blocked(),
        playing_app: msg.playing_app(),
    };
    tracing::debug!(?state, "playing session state");
    *PLAYING.lock().expect("playing lock poisoned") = Some((session_id, state));
}

/// Allow invites only into running games.
pub fn check_invite(state: Option<PlayingSession>, app_id: u32) -> Result<()> {
    match state {
        Some(s) if s.playing_app == app_id => Ok(()),
        Some(s) if s.playing_app != 0 => bail!(
            "your account is playing app {} right now, not app {app_id}; Steam takes the \
             invite's game from the running session, so invite to app {} instead",
            s.playing_app,
            s.playing_app
        ),
        Some(_) => bail!(
            "app {app_id} isn't running on your account; start it in Steam (or with \
             `aurelia play {app_id} --steam`) and invite from there"
        ),
        None => bail!(
            "Steam didn't report whether a game is running, so the invite wasn't sent; \
             try again in a few seconds"
        ),
    }
}

/// One message from a friend conversation, for the history view.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatMessage {
    /// SteamID64 of the sender.
    pub sender: u64,
    /// Whether the logged-in user sent this message.
    pub from_self: bool,
    /// Message body.
    pub message: String,
    /// Unix timestamp (seconds) the message was sent.
    pub timestamp: u32,
}

/// Outcome of sending a chat message.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SentMessage {
    /// Server-side send time (unix seconds); 0 if Steam did not report one.
    pub server_timestamp: u64,
    /// The message as the server stored it (e.g. bbcode applied), if it differs
    /// from what was sent.
    pub modified_message: Option<String>,
}

impl SteamClient {
    /// Build a chat client over a clone of the shared connection.
    ///
    /// steam-vent multiplexes jobs over one connection, so the clone is cheap and
    /// shares the daemon's live session. Callers that want to receive events hold
    /// the returned [`ChatClient`] for the lifetime of the chat session and poll
    /// [`ChatClient::listen`] on it — the event stream is type-tied to the client,
    /// so it must outlive the stream.
    pub fn chat_client(&self) -> Result<ChatClient> {
        let connection = self.require_connection_owned()?;
        Ok(ChatClient::new(connection))
    }

    /// Send a direct chat message to a friend (by SteamID64).
    pub async fn send_chat_message(&self, target: u64, message: String) -> Result<SentMessage> {
        let target =
            SteamID::try_from(target).map_err(|_| anyhow!("invalid target SteamID: {target}"))?;
        let result = self
            .chat_client()?
            .send_message(target, message)
            .await
            .map_err(|e| anyhow!("failed to send chat message: {e}"))?;
        Ok(SentMessage {
            server_timestamp: result.server_timestamp,
            modified_message: result.modified_message,
        })
    }

    /// Fetch recent messages exchanged with a friend (by SteamID64), in the order
    /// Steam returns them (most-recent first).
    pub async fn recent_chat_messages(&self, target: u64, count: u32) -> Result<Vec<ChatMessage>> {
        let connection = self.require_connection()?;
        let me = u64::from(connection.steam_id());
        // FriendMessage carries only the 32-bit account id of the sender; compare
        // it against our own to tell our messages from the friend's. In a 1:1
        // conversation any non-self message is from the friend.
        let me_account = (me & 0xFFFF_FFFF) as u32;

        let mut req = CFriendMessages_GetRecentMessages_Request::new();
        req.set_steamid1(me);
        req.set_steamid2(target);
        req.set_count(count);
        req.set_most_recent_conversation(false);

        let resp: CFriendMessages_GetRecentMessages_Response = connection
            .service_method(req)
            .await
            .map_err(|e| anyhow!("failed to fetch recent messages: {e}"))?;

        let messages = resp
            .messages
            .into_iter()
            .map(|m| {
                let from_self = m.accountid() == me_account;
                ChatMessage {
                    sender: if from_self { me } else { target },
                    from_self,
                    message: m.message().to_string(),
                    timestamp: m.timestamp(),
                }
            })
            .collect();
        Ok(messages)
    }

    /// Last playing state for this connection.
    pub fn playing_session(&self) -> Option<PlayingSession> {
        let session = self.connection.as_ref()?.session_id();
        match *PLAYING.lock().expect("playing lock poisoned") {
            Some((s, state)) if s == session => Some(state),
            _ => None,
        }
    }

    /// Playing state, waiting briefly if unknown.
    pub async fn fetch_playing_session(&self) -> Option<PlayingSession> {
        if let Some(state) = self.playing_session() {
            return Some(state);
        }
        let connection = self.connection.as_ref()?.clone();
        let session = connection.session_id();
        let mut stream = connection.on::<CMsgClientPlayingSessionState>();
        // Logon push may sit unclaimed.
        for raw in connection.take_unprocessed() {
            if raw.kind != <CMsgClientPlayingSessionState as ReceivableMessage>::KIND {
                super::auth_ticket::absorb_unprocessed(session, raw);
            } else if let Ok(msg) = raw.into_message::<CMsgClientPlayingSessionState>() {
                record_playing_session(session, &msg);
            }
        }
        if let Some(state) = self.playing_session() {
            return Some(state);
        }
        if let Ok(Some(Ok(msg))) = tokio::time::timeout(PLAYING_STATE_WAIT, stream.next()).await {
            record_playing_session(session, &msg);
        }
        self.playing_session()
    }

    /// Invite a friend into a running game.
    pub async fn invite_to_game(
        &self,
        target: u64,
        app_id: u32,
        connect: Option<String>,
    ) -> Result<()> {
        SteamID::try_from(target).map_err(|_| anyhow!("invalid target SteamID: {target}"))?;
        let connection = self.require_connection()?;
        let me = u64::from(connection.steam_id());

        // Never report a game ourselves.
        check_invite(self.fetch_playing_session().await, app_id)?;

        let mut invite = CMsgClientInviteToGame::new();
        invite.set_steam_id_dest(target);
        invite.set_steam_id_src(me);
        invite.set_connect_string(connect.unwrap_or_default());
        connection
            .send(invite)
            .await
            .map_err(|e| anyhow!("failed to send game invite: {e}"))
    }
}

#[cfg(test)]
mod invite_tests {
    use super::*;

    fn state(blocked: bool, playing_app: u32) -> Option<PlayingSession> {
        Some(PlayingSession {
            blocked,
            playing_app,
        })
    }

    #[test]
    fn steam_client_running_the_app_is_allowed() {
        assert!(check_invite(state(true, 1942280), 1942280).is_ok());
    }

    #[test]
    fn this_session_running_the_app_is_allowed() {
        assert!(check_invite(state(false, 1942280), 1942280).is_ok());
    }

    #[test]
    fn other_running_app_is_refused() {
        let err = check_invite(state(true, 440), 1942280)
            .unwrap_err()
            .to_string();
        assert!(err.contains("app 440"), "{err}");
    }

    #[test]
    fn nothing_running_is_refused() {
        let err = check_invite(state(false, 0), 1942280)
            .unwrap_err()
            .to_string();
        assert!(err.contains("isn't running"), "{err}");
    }

    #[test]
    fn blocked_without_app_is_refused() {
        assert!(check_invite(state(true, 0), 1942280).is_err());
    }

    #[test]
    fn unknown_state_is_refused() {
        assert!(check_invite(None, 1942280).is_err());
    }
}
