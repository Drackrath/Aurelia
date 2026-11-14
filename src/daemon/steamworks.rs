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

use aurelia::steam_client::auth_ticket::{AuthSessionTicket, TicketAuthEvent, TicketKind};
use aurelia::steam_client::playing::{self, GAMES, Launch};
use aurelia::steam_client::remote_storage::{RemoteStorageInfo, remote_storage_info};
use aurelia::steam_client::steamworks_info::{AppInstallInfo, DlcInfo};
use aurelia::steam_client::{EncryptedTicketOutcome, SteamClient};

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
}

/// Cloud info flag bits.
pub mod cloud_flag {
    pub const ENABLED: u8 = 1;
    pub const WRITABLE: u8 = 2;
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
    /// Lobby pushes for this app.
    fn lobby_events(
        &self,
        app_id: u32,
    ) -> impl Future<Output = Option<mpsc::UnboundedReceiver<LobbyEvent>>> + Send;
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
    let joined: Vec<u64> = conn.lobbies.lock().unwrap().drain().collect();
    for lobby in joined {
        if let Err(e) = conn.backend.leave_lobby(hello.app_id, lobby).await {
            tracing::warn!(lobby, "steamworks: lobby leave failed: {e:#}");
        }
    }
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
    let Some(steam_id) = steam_id else {
        return (status::NO_SESSION, b"not logged in".to_vec());
    };
    let mut f = Fields::new(&req.body);
    match req.command {
        cmd::OWNERSHIP_TICKET => match conn.backend.ownership_ticket(app_id).await {
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
        let mut info = client.app_install_info(app_id).await?;
        if info.language.is_none() {
            info.language = Some(crate::commands::common::resolve_steam_language(None).await);
        }
        Ok(info)
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
