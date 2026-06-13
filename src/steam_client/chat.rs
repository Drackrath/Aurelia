//! Friend chat: send and receive direct messages over the shared Steam
//! connection, and fetch recent message history.
//!
//! The live send/receive path wraps the vendored `steam-vent-chat` `ChatClient`
//! (see `vendor/steam-vent-chat`); history uses the `FriendMessages`
//! `GetRecentMessages` service method directly. As with the other `SteamClient`
//! submodules, the struct and shared imports live in the parent module and are
//! pulled in via `use super::*`.
use super::*;
use steam_vent_chat::ChatClient;
use steam_vent_proto::steammessages_friendmessages_steamclient::{
    CFriendMessages_GetRecentMessages_Request, CFriendMessages_GetRecentMessages_Response,
};
use steamid_ng::SteamID;

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
        let connection = self
            .connection
            .as_ref()
            .cloned()
            .context("steam connection not initialized")?;
        Ok(ChatClient::new(connection))
    }

    /// Send a direct chat message to a friend (by SteamID64).
    pub async fn send_chat_message(&self, target: u64, message: String) -> Result<SentMessage> {
        let target = SteamID::try_from(target)
            .map_err(|_| anyhow!("invalid target SteamID: {target}"))?;
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
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
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
}
