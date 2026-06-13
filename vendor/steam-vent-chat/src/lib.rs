use steam_vent_proto::steammessages_friendmessages_steamclient::{
    CFriendMessages_IncomingMessage_Notification, CFriendMessages_SendMessage_Request,
};
use steam_vent::{Connection, ConnectionTrait, NetworkError};
use steamid_ng::SteamID;
use tokio_stream::{Stream, StreamExt};

/// High level wrapper around steam-vent's [`Connection`] for implementing a chat client
pub struct ChatClient {
    connection: Connection,
}

impl ChatClient {
    /// Create a chat client from a [`Connection`]
    pub fn new(connection: Connection) -> Self {
        ChatClient { connection }
    }

    /// Listen for incoming events
    pub fn listen(&self) -> impl Stream<Item = Result<ChatEvent, NetworkError>> + 'static {
        self.connection
            .on_notification::<CFriendMessages_IncomingMessage_Notification>()
            .filter_map(|notification| {
                notification
                    .map(|notification| ChatEvent::try_from(notification).ok())
                    .transpose()
            })
    }

    /// Send a chat message to a user
    pub async fn send_message(&self, target: SteamID, message: String) -> Result<MessageResult, NetworkError> {
        let req = CFriendMessages_SendMessage_Request {
            steamid: Some(target.into()),
            message: Some(message),
            chat_entry_type: Some(MessageType::Chat as i32),
            ..CFriendMessages_SendMessage_Request::default()
        };
        let result = self.connection.service_method(req).await?;

        Ok(MessageResult {
            server_timestamp: result.server_timestamp() as u64,
            modified_message: result.modified_message,
        })
    }
}

/// Incoming chat event
#[derive(Debug)]
pub enum ChatEvent {
    /// Another user sent a message
    Message(MessageEvent),
    /// The local user sent a message from another device
    EchoMessage(MessageEvent),
    /// Another user is typing
    Typing(TypingEvent),
}

impl TryFrom<CFriendMessages_IncomingMessage_Notification> for ChatEvent {
    type Error = ();

    fn try_from(
        notification: CFriendMessages_IncomingMessage_Notification,
    ) -> Result<Self, Self::Error> {
        // steamid-ng 3.x dropped `Default` for `SteamID`, so an unparseable id can
        // no longer fall back to a zero id — drop the event instead.
        let source = SteamID::try_from(notification.steamid_friend()).map_err(|_| ())?;
        let message_type =
            MessageType::try_from(notification.chat_entry_type()).unwrap_or_default();
        Ok(match message_type {
            // A message from the friend (not a local echo of our own send). The
            // upstream code guarded both arms on `local_echo()`, which made the
            // echo arm unreachable and silently dropped every genuine incoming
            // message; the real distinction is whether `local_echo` is set.
            MessageType::Chat if !notification.local_echo() => ChatEvent::Message(MessageEvent {
                source,
                server_timestamp: notification.rtime32_server_timestamp() as u64,
                message: notification.message.unwrap_or_default(),
                message_no_bbcode: notification.message_no_bbcode,
            }),
            MessageType::Chat => ChatEvent::EchoMessage(MessageEvent {
                source,
                server_timestamp: notification.rtime32_server_timestamp() as u64,
                message: notification.message.unwrap_or_default(),
                message_no_bbcode: notification.message_no_bbcode,
            }),
            MessageType::Typing => ChatEvent::Typing(TypingEvent {
                source,
                server_timestamp: notification.rtime32_server_timestamp() as u64,
            }),
            _ => return Err(()),
        })
    }
}

/// Incoming chat message
#[derive(Debug)]
pub struct MessageEvent {
    /// SteamID of the sender
    pub source: SteamID,
    /// Raw message contents
    pub message: String,
    /// Message contents without any bbcode markup
    pub message_no_bbcode: Option<String>,
    /// Service side time when the message was sent
    pub server_timestamp: u64,
}

/// Incoming typing event
#[derive(Debug)]
pub struct TypingEvent {
    /// SteamID of the typer
    pub source: SteamID,
    /// Service side time when the user was typing
    pub server_timestamp: u64,
}

#[repr(i32)]
#[derive(Default)]
enum MessageType {
    #[default]
    Invalid,
    Chat = 1,
    Typing,
    GameInvite,
    Left = 6,
    Entered,
    Kicked,
    Banned,
    Disconnected,
    Historical,
    LinkBlocked = 14,
}

impl TryFrom<i32> for MessageType {
    type Error = ();

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        Ok(match value {
            1 => MessageType::Chat,
            2 => MessageType::Typing,
            3 => MessageType::GameInvite,
            6 => MessageType::Left,
            7 => MessageType::Entered,
            8 => MessageType::Kicked,
            9 => MessageType::Banned,
            10 => MessageType::Disconnected,
            11 => MessageType::Historical,
            14 => MessageType::LinkBlocked,
            _ => return Err(()),
        })
    }
}

/// Result of a sent chat message
#[derive(Debug)]
pub struct MessageResult {
    /// Chat message with bbcode added
    pub modified_message: Option<String>,
    pub server_timestamp: u64,
}