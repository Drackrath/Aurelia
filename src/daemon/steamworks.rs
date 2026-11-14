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
use aurelia::steam_client::microtxn::{self, MicroTxnRequest};
use aurelia::steam_client::playing::{self, GAMES, Launch};
use aurelia::steam_client::steamworks_info::{AppInstallInfo, DlcInfo};
use aurelia::steam_client::{EncryptedTicketOutcome, Friend, SteamClient};

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
    pub const ENCRYPTED_APP_TICKET: u8 = 5;
    pub const DLC_LIST: u8 = 6;
    pub const APP_INFO: u8 = 7;
    pub const SESSION_INFO: u8 = 8;
    pub const FRIENDS: u8 = 50;
    pub const SET_RICH_PRESENCE: u8 = 51;
    pub const FRIEND_RICH_PRESENCE: u8 = 52;
    pub const OPEN_URL: u8 = 53;
}

/// Rich presence limits Steam enforces.
pub mod rich_presence {
    pub const MAX_KEYS: usize = 30;
    pub const MAX_KEY_LEN: usize = 64;
    pub const MAX_VALUE_LEN: usize = 256;
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
    if !matches!(
        req.command,
        cmd::OWNERSHIP_TICKET
            | cmd::CREATE_AUTH_TICKET
            | cmd::CANCEL_AUTH_TICKET
            | cmd::ENCRYPTED_APP_TICKET
            | cmd::DLC_LIST
            | cmd::APP_INFO
            | cmd::SESSION_INFO
            | cmd::FRIENDS
            | cmd::SET_RICH_PRESENCE
            | cmd::FRIEND_RICH_PRESENCE
            | cmd::OPEN_URL
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

    async fn open_url(&self, url: String) -> Result<bool> {
        Ok(tokio::task::spawn_blocking(move || spawn_browser(&url)).await?)
    }

    async fn session_info(&self) -> Result<(u32, String)> {
        let client = authed_client().await.ok_or_else(not_logged_in)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX));
        Ok((now, client.ip_country().unwrap_or_default()))
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
