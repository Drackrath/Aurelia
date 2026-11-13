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

use aurelia::steam_client::SteamClient;
use aurelia::steam_client::auth_ticket::{AuthSessionTicket, TicketAuthEvent, TicketKind};
use aurelia::steam_client::playing::{self, GAMES, Launch};

use super::proto::{self, SharedWriter};

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
}

/// Pushed message kinds.
pub mod push {
    pub const TICKET_AUTH: u8 = 1;
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

    #[cfg(test)]
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

/// What the daemon offers games.
pub trait Backend: Send + Sync + 'static {
    /// SteamID when logged on.
    fn steam_id(&self) -> impl Future<Output = Option<u64>> + Send;
    /// Report playing; returns launch id.
    fn start(&self, app_id: u32, pid: Option<u32>) -> impl Future<Output = Result<u64>> + Send;
    /// Clear one launch.
    fn stop(&self, launch: u64) -> impl Future<Output = ()> + Send;
    fn ownership_ticket(&self, app_id: u32) -> impl Future<Output = Result<Vec<u8>>> + Send;
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
}

/// Routes pushes to owning connections.
#[derive(Default)]
pub struct Router {
    owners: Mutex<HashMap<u32, mpsc::UnboundedSender<Vec<u8>>>>,
}

impl Router {
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
    let conn = Arc::new(Conn {
        backend,
        router,
        app_id: hello.app_id,
        offline,
        owned: Mutex::new(HashSet::new()),
        push_tx,
    });
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
    if !matches!(
        req.command,
        cmd::OWNERSHIP_TICKET | cmd::CREATE_AUTH_TICKET | cmd::CANCEL_AUTH_TICKET
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
                    Ok(ticket) => (status::OK, ticket),
                    Err(e) => failed(&e),
                }
            }
            _ => (status::NO_SESSION, b"offline".to_vec()),
        };
    }
    if steam_id.is_none() {
        return (status::NO_SESSION, b"not logged in".to_vec());
    }
    let mut f = Fields::new(&req.body);
    match req.command {
        cmd::OWNERSHIP_TICKET => match conn.backend.ownership_ticket(app_id).await {
            Ok(ticket) => (status::OK, ticket),
            Err(e) => failed(&e),
        },
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
}

#[cfg(test)]
#[path = "steamworks_tests.rs"]
mod tests;
