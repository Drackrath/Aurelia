//! Auth session tickets for games.
use super::*;
use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex;
use steam_vent::ReceivableMessage;
use steam_vent_proto::steammessages_base::CMsgAuthTicket;
use steam_vent_proto::steammessages_clientserver::{
    CMsgClientAuthList, CMsgClientAuthListAck, CMsgClientGameConnectTokens,
    CMsgClientTicketAuthComplete,
};
use steam_vent_proto::steammessages_clientserver_login::CMsgClientLoggedOff;
use tokio::sync::Notify;
use tokio_stream::StreamExt;

/// Session header type: auth session.
pub const TICKET_TYPE_SESSION: u32 = 2;
/// Session header type: web API.
pub const TICKET_TYPE_WEB_API: u32 = 5;
/// Session header byte length.
pub const SESSION_HEADER_LEN: u32 = 24;
/// GC token byte length.
pub const GC_TOKEN_LEN: usize = 20;
/// Registered part: token plus header.
pub const AUTH_PART_LEN: usize = 4 + GC_TOKEN_LEN + 4 + SESSION_HEADER_LEN as usize;
/// EAuthSessionResponse OK.
const AUTH_SESSION_RESPONSE_OK: u32 = 0;
/// Pending events kept per session.
const MAX_EVENTS: usize = 64;
/// Wait for tokens or acks.
const STEAM_WAIT: Duration = Duration::from_secs(10);

/// Values of the session header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionHeader {
    /// 2 session, 5 web API.
    pub ticket_type: u32,
    /// Public IPv4 seen by Steam.
    pub external_ip: Ipv4Addr,
    /// Milliseconds since CM connect.
    pub connected_ms: u32,
    /// Tickets built this session.
    pub connection_count: u32,
}

/// Build the AuthList-registered part.
pub fn build_auth_part(gc_token: &[u8], header: &SessionHeader) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + gc_token.len() + 4 + SESSION_HEADER_LEN as usize);
    put_u32(&mut out, gc_token.len() as u32);
    out.extend_from_slice(gc_token);
    put_u32(&mut out, SESSION_HEADER_LEN);
    put_u32(&mut out, 1);
    put_u32(&mut out, header.ticket_type);
    put_u32(&mut out, u32::from(header.external_ip));
    put_u32(&mut out, 0);
    put_u32(&mut out, header.connected_ms);
    put_u32(&mut out, header.connection_count);
    out
}

/// Append length-prefixed ownership ticket.
pub fn build_session_ticket(auth_part: &[u8], ownership_ticket: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(auth_part.len() + 4 + ownership_ticket.len());
    out.extend_from_slice(auth_part);
    put_u32(&mut out, ownership_ticket.len() as u32);
    out.extend_from_slice(ownership_ticket);
    out
}

/// CRC32 Steam expects per ticket.
pub fn ticket_crc(auth_part: &[u8]) -> u32 {
    crc32fast::hash(auth_part)
}

/// A session ticket split apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSessionTicket<'a> {
    pub gc_token: &'a [u8],
    pub header: SessionHeader,
    pub auth_part: &'a [u8],
    pub ownership_ticket: &'a [u8],
}

/// Split a ticket; None if malformed.
pub fn parse_session_ticket(ticket: &[u8]) -> Option<ParsedSessionTicket<'_>> {
    let token_len = read_u32(ticket, 0)? as usize;
    if token_len != GC_TOKEN_LEN {
        return None;
    }
    let mut at = 4 + token_len;
    let gc_token = ticket.get(4..at)?;
    if read_u32(ticket, at)? != SESSION_HEADER_LEN {
        return None;
    }
    at += 4;
    let header = SessionHeader {
        ticket_type: read_u32(ticket, at + 4)?,
        external_ip: Ipv4Addr::from(read_u32(ticket, at + 8)?),
        connected_ms: read_u32(ticket, at + 16)?,
        connection_count: read_u32(ticket, at + 20)?,
    };
    at += SESSION_HEADER_LEN as usize;
    let auth_part = ticket.get(..at)?;
    let owner_len = read_u32(ticket, at)? as usize;
    at += 4;
    if ticket.len() != at.checked_add(owner_len)? {
        return None;
    }
    Some(ParsedSessionTicket {
        gc_token,
        header,
        auth_part,
        ownership_ticket: &ticket[at..],
    })
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn read_u32(buf: &[u8], at: usize) -> Option<u32> {
    let bytes = buf.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// IPv4 for the header, else zero.
pub fn header_ip(ip: IpAddr) -> Ipv4Addr {
    match ip {
        IpAddr::V4(v4) => v4,
        IpAddr::V6(v6) => v6.to_ipv4_mapped().unwrap_or(Ipv4Addr::UNSPECIFIED),
    }
}

/// Which ticket a game asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketKind {
    /// GetAuthSessionTicket.
    Session,
    /// GetAuthTicketForWebApi with identity.
    WebApi(Option<String>),
}

impl TicketKind {
    fn header_type(&self) -> u32 {
        match self {
            Self::Session => TICKET_TYPE_SESSION,
            Self::WebApi(_) => TICKET_TYPE_WEB_API,
        }
    }

    /// Identity bytes Steam binds to.
    pub fn server_secret(&self) -> Option<Vec<u8>> {
        match self {
            Self::WebApi(Some(id)) if !id.is_empty() => Some(format!("str:{id}\0").into_bytes()),
            _ => None,
        }
    }
}

/// One ticket listed in ClientAuthList.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTicket {
    /// HAuthTicket given to the game.
    pub handle: u32,
    pub app_id: u32,
    /// Zero for our own tickets.
    pub steam_id: u64,
    pub pipe: u32,
    pub crc: u32,
    pub auth_part: Vec<u8>,
    pub estate: u32,
    pub server_secret: Option<Vec<u8>>,
}

/// Steam's verdict on a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketAuthEvent {
    pub handle: Option<u32>,
    pub steam_id: u64,
    pub owner_steam_id: u64,
    pub game_id: u64,
    pub ticket_crc: u32,
    pub estate: u32,
    pub response: u32,
}

/// Per-session token and ticket state.
#[derive(Debug)]
pub struct AuthTickets {
    tokens: VecDeque<Vec<u8>>,
    tickets: Vec<ActiveTicket>,
    events: VecDeque<TicketAuthEvent>,
    next_handle: u32,
    seq_me: u32,
    seq_them: u32,
    connection_count: u32,
    connected_at: Instant,
}

impl AuthTickets {
    pub fn new(connected_at: Instant) -> Self {
        Self {
            tokens: VecDeque::new(),
            tickets: Vec::new(),
            events: VecDeque::new(),
            next_handle: 1,
            seq_me: 0,
            seq_them: 0,
            connection_count: 0,
            connected_at,
        }
    }

    /// Store pushed GC tokens.
    pub fn absorb_tokens(&mut self, msg: &CMsgClientGameConnectTokens) {
        self.tokens.extend(msg.tokens.iter().cloned());
        let keep = msg.max_tokens_to_keep() as usize;
        if msg.has_max_tokens_to_keep() && keep > 0 {
            while self.tokens.len() > keep {
                self.tokens.pop_front();
            }
        }
    }

    pub fn tokens_left(&self) -> usize {
        self.tokens.len()
    }

    pub fn clear_tokens(&mut self) {
        self.tokens.clear();
    }

    pub fn tickets(&self) -> &[ActiveTicket] {
        &self.tickets
    }

    /// Header for the next ticket.
    pub fn next_header(
        &mut self,
        kind: &TicketKind,
        external_ip: Ipv4Addr,
        now: Instant,
    ) -> SessionHeader {
        self.connection_count = self.connection_count.wrapping_add(1);
        let elapsed = now.saturating_duration_since(self.connected_at).as_millis();
        SessionHeader {
            ticket_type: kind.header_type(),
            external_ip,
            connected_ms: u32::try_from(elapsed).unwrap_or(u32::MAX),
            connection_count: self.connection_count,
        }
    }

    /// Take a token, build, register.
    pub fn issue(
        &mut self,
        app_id: u32,
        pipe: u32,
        kind: &TicketKind,
        external_ip: Ipv4Addr,
        now: Instant,
    ) -> Option<ActiveTicket> {
        let token = self.tokens.pop_front()?;
        let header = self.next_header(kind, external_ip, now);
        let auth_part = build_auth_part(&token, &header);
        let handle = self.next_handle;
        self.next_handle = self.next_handle.wrapping_add(1).max(1);
        let ticket = ActiveTicket {
            handle,
            app_id,
            steam_id: 0,
            pipe,
            crc: ticket_crc(&auth_part),
            auth_part,
            estate: 0,
            server_secret: kind.server_secret(),
        };
        self.tickets.push(ticket.clone());
        Some(ticket)
    }

    /// Drop a ticket; returns its app.
    pub fn cancel(&mut self, handle: u32) -> Option<u32> {
        let at = self
            .tickets
            .iter()
            .position(|t| t.steam_id == 0 && t.handle == handle)?;
        Some(self.tickets.remove(at).app_id)
    }

    /// Next ClientAuthList message.
    pub fn auth_list(&mut self, force_app: Option<u32>) -> CMsgClientAuthList {
        let mut app_ids: Vec<u32> = Vec::new();
        for app in self.tickets.iter().map(|t| t.app_id).chain(force_app) {
            if !app_ids.contains(&app) {
                app_ids.push(app);
            }
        }
        let mut msg = CMsgClientAuthList::new();
        msg.set_tokens_left(self.tokens.len() as u32);
        msg.set_last_request_seq(self.seq_me);
        msg.set_last_request_seq_from_server(self.seq_them);
        msg.tickets = self.tickets.iter().map(to_proto).collect();
        msg.app_ids = app_ids;
        self.seq_me = self.seq_me.wrapping_add(1);
        msg.set_message_sequence(self.seq_me);
        msg
    }

    /// Record Steam's ack sequence.
    pub fn on_ack(&mut self, ack: &CMsgClientAuthListAck) {
        if ack.has_message_sequence() {
            self.seq_them = ack.message_sequence();
        }
    }

    /// Apply ClientTicketAuthComplete.
    pub fn on_complete(&mut self, msg: &CMsgClientTicketAuthComplete) -> TicketAuthEvent {
        let crc = msg.ticket_crc();
        let mut handle = None;
        if let Some(at) = self.tickets.iter().position(|t| t.crc == crc) {
            handle = Some(self.tickets[at].handle);
            self.tickets[at].estate = msg.estate();
            if msg.eauth_session_response() != AUTH_SESSION_RESPONSE_OK {
                self.tickets.remove(at);
            }
        }
        let event = TicketAuthEvent {
            handle,
            steam_id: msg.steam_id(),
            owner_steam_id: msg.owner_steam_id(),
            game_id: msg.game_id(),
            ticket_crc: crc,
            estate: msg.estate(),
            response: msg.eauth_session_response(),
        };
        if self.events.len() == MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(event.clone());
        event
    }

    /// Drain queued verdicts.
    pub fn take_events(&mut self) -> Vec<TicketAuthEvent> {
        self.events.drain(..).collect()
    }
}

fn to_proto(t: &ActiveTicket) -> CMsgAuthTicket {
    let mut p = CMsgAuthTicket::new();
    p.set_estate(t.estate);
    p.set_steamid(t.steam_id);
    p.set_gameid(u64::from(t.app_id));
    p.set_h_steam_pipe(t.pipe);
    p.set_ticket_crc(t.crc);
    p.set_ticket(t.auth_part.clone());
    if let Some(secret) = &t.server_secret {
        p.set_server_secret(secret.clone());
    }
    p
}

/// State for the live session.
static STORE: Mutex<Option<(i32, AuthTickets)>> = Mutex::new(None);
/// Wakes token waiters.
static TOKENS_ARRIVED: Notify = Notify::const_new();

/// Run `f` on the session's state.
fn with_store<R>(session: i32, f: impl FnOnce(&mut AuthTickets) -> R) -> R {
    let mut guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.as_ref().is_none_or(|(id, _)| *id != session) {
        *guard = Some((session, AuthTickets::new(Instant::now())));
    }
    let (_, state) = guard.as_mut().expect("store just set");
    f(state)
}

/// Store tokens pushed to `session`.
pub(crate) fn record_gc_tokens(session: i32, msg: &CMsgClientGameConnectTokens) {
    let left = with_store(session, |s| {
        s.absorb_tokens(msg);
        s.tokens_left()
    });
    tracing::debug!(left, "game connect tokens received");
    TOKENS_ARRIVED.notify_waiters();
}

/// Keep tokens from unclaimed messages.
pub(crate) fn absorb_unprocessed(session: i32, raw: steam_vent::RawNetMessage) {
    if raw.kind == <CMsgClientGameConnectTokens as ReceivableMessage>::KIND
        && let Ok(msg) = raw.into_message::<CMsgClientGameConnectTokens>()
    {
        record_gc_tokens(session, &msg);
    }
}

/// A ticket ready for the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSessionTicket {
    pub handle: u32,
    pub app_id: u32,
    pub crc: u32,
    /// Full ticket bytes for the game.
    pub ticket: Vec<u8>,
    /// Steam acked the AuthList.
    pub acknowledged: bool,
}

impl SteamClient {
    /// Track tokens, verdicts and acks.
    pub async fn run_auth_ticket_watcher(&self) -> Result<()> {
        let connection = self.require_connection_owned()?;
        let session = connection.session_id();
        with_store(session, |_| ());
        let mut tokens = connection.on::<CMsgClientGameConnectTokens>();
        let mut complete = connection.on::<CMsgClientTicketAuthComplete>();
        let mut acks = connection.on::<CMsgClientAuthListAck>();
        let mut logoff = connection.on::<CMsgClientLoggedOff>();
        loop {
            tokio::select! {
                Some(item) = tokens.next() => {
                    if let Ok(msg) = item {
                        record_gc_tokens(session, &msg);
                    }
                }
                Some(item) = complete.next() => {
                    if let Ok(msg) = item {
                        let event = with_store(session, |s| s.on_complete(&msg));
                        tracing::debug!(?event, "ticket auth complete");
                    }
                }
                Some(item) = acks.next() => {
                    if let Ok(ack) = item {
                        with_store(session, |s| s.on_ack(&ack));
                    }
                }
                Some(_) = logoff.next() => {
                    with_store(session, AuthTickets::clear_tokens);
                }
                else => return Ok(()),
            }
        }
    }

    /// Build and register a ticket.
    pub async fn create_auth_ticket(
        &self,
        app_id: u32,
        pipe: u32,
        kind: TicketKind,
    ) -> Result<AuthSessionTicket> {
        let connection = self.require_connection_owned()?;
        let session = connection.session_id();
        let ownership = self.get_app_ticket(app_id).await?;
        let ip = header_ip(connection.public_ip());

        for raw in connection.take_unprocessed() {
            absorb_unprocessed(session, raw);
        }
        let deadline = tokio::time::Instant::now() + STEAM_WAIT;
        let issued = loop {
            let arrived = TOKENS_ARRIVED.notified();
            tokio::pin!(arrived);
            arrived.as_mut().enable();
            let issued = with_store(session, |s| {
                s.issue(app_id, pipe, &kind, ip, Instant::now())
            });
            if let Some(ticket) = issued {
                break ticket;
            }
            if tokio::time::timeout_at(deadline, arrived).await.is_err() {
                bail!("Steam sent no game connect tokens; no auth ticket for app {app_id}");
            }
        };

        let list = with_store(session, |s| s.auth_list(None));
        let crc = issued.crc;
        let acknowledged = send_auth_list(&connection, session, list, move |ack| {
            ack.ticket_crc.contains(&crc)
        })
        .await?;
        Ok(AuthSessionTicket {
            handle: issued.handle,
            app_id,
            crc,
            ticket: build_session_ticket(&issued.auth_part, &ownership),
            acknowledged,
        })
    }

    /// Cancel a ticket; false if unknown.
    pub async fn cancel_auth_ticket(&self, handle: u32) -> Result<bool> {
        let connection = self.require_connection_owned()?;
        let session = connection.session_id();
        let Some((list, crc)) = with_store(session, |s| {
            let crc = s.tickets().iter().find(|t| t.handle == handle)?.crc;
            let app = s.cancel(handle)?;
            Some((s.auth_list(Some(app)), crc))
        }) else {
            return Ok(false);
        };
        send_auth_list(&connection, session, list, move |ack| {
            !ack.ticket_crc.contains(&crc)
        })
        .await?;
        Ok(true)
    }

    /// Steam verdicts since last call.
    pub fn take_ticket_auth_events(&self) -> Vec<TicketAuthEvent> {
        match self.connection.as_ref() {
            Some(c) => with_store(c.session_id(), AuthTickets::take_events),
            None => Vec::new(),
        }
    }
}

/// Send AuthList; wait for matching ack.
async fn send_auth_list(
    connection: &Connection,
    session: i32,
    list: CMsgClientAuthList,
    matches: impl Fn(&CMsgClientAuthListAck) -> bool,
) -> Result<bool> {
    let mut acks = connection.on::<CMsgClientAuthListAck>();
    // Steam may skip the job id.
    let job = connection.job::<CMsgClientAuthList, CMsgClientAuthListAck>(list);
    tokio::pin!(job);
    let deadline = tokio::time::sleep(STEAM_WAIT);
    tokio::pin!(deadline);
    let mut job_done = false;
    loop {
        let ack = tokio::select! {
            res = &mut job, if !job_done => {
                job_done = true;
                match res {
                    Ok(ack) => ack,
                    Err(steam_vent::NetworkError::Timeout) => continue,
                    Err(e) => return Err(anyhow!(e).context("failed sending ClientAuthList")),
                }
            }
            Some(Ok(ack)) = acks.next() => ack,
            () = &mut deadline => {
                tracing::warn!("ClientAuthList not acknowledged in time");
                return Ok(false);
            }
        };
        with_store(session, |s| s.on_ack(&ack));
        if matches(&ack) {
            return Ok(true);
        }
    }
}

#[cfg(test)]
#[path = "auth_ticket_tests.rs"]
mod tests;
