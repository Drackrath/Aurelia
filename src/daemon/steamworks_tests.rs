use super::*;
use anyhow::bail;
use tokio::io::DuplexStream;

/// Rich presence pairs.
type Pairs = Vec<(String, String)>;

/// Scriptable daemon state.
#[derive(Default)]
struct Fake {
    steam_id: Mutex<Option<u64>>,
    conflict: bool,
    started: Mutex<Vec<(u32, Option<u32>)>>,
    stopped: Mutex<Vec<u64>>,
    cancelled: Mutex<Vec<u32>>,
    events: Mutex<Vec<TicketAuthEvent>>,
    offline_id: Option<u64>,
    cached: HashMap<u32, Vec<u8>>,
    left: Mutex<Vec<u64>>,
    lobby_tx: Mutex<Option<mpsc::UnboundedSender<LobbyEvent>>>,
    lobby_calls: Mutex<Vec<String>>,
    writes: bool,
    stored: Mutex<Vec<(u32, u32, bool, StatChanges)>>,
    presence: Mutex<Vec<(u32, Pairs)>>,
    opened: Mutex<Vec<String>>,
    queries: Mutex<Vec<UgcQuery>>,
    subscribed: Mutex<Vec<u64>>,
}

const SID: u64 = 76_561_197_960_287_930;

impl Fake {
    fn logged_in() -> Arc<Self> {
        Arc::new(Self {
            steam_id: Mutex::new(Some(SID)),
            ..Self::default()
        })
    }
}

impl Backend for Fake {
    async fn steam_id(&self) -> Option<u64> {
        *self.steam_id.lock().unwrap()
    }

    async fn start(&self, app_id: u32, pid: Option<u32>) -> Result<u64> {
        if self.conflict {
            bail!("another client is playing");
        }
        self.started.lock().unwrap().push((app_id, pid));
        Ok(7)
    }

    async fn stop(&self, launch: u64) {
        self.stopped.lock().unwrap().push(launch);
    }

    async fn ownership_ticket(&self, app_id: u32) -> Result<Vec<u8>> {
        if app_id == 480 {
            Ok(vec![0xAA, 0xBB])
        } else {
            bail!("app {app_id} not owned")
        }
    }

    async fn create_auth_ticket(
        &self,
        app_id: u32,
        pipe: u32,
        kind: TicketKind,
    ) -> Result<AuthSessionTicket> {
        let handle = match kind {
            TicketKind::Session => 100 + pipe,
            TicketKind::WebApi(Some(_)) => 200,
            TicketKind::WebApi(None) => 300,
        };
        Ok(AuthSessionTicket {
            handle,
            app_id,
            crc: 0xDEAD_BEEF,
            ticket: vec![1, 2, 3],
            acknowledged: true,
        })
    }

    async fn cancel_auth_ticket(&self, handle: u32) -> Result<bool> {
        self.cancelled.lock().unwrap().push(handle);
        Ok(true)
    }

    async fn ticket_events(&self) -> Vec<TicketAuthEvent> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }

    async fn offline_steam_id(&self) -> Option<u64> {
        self.offline_id
    }

    async fn cached_ownership_ticket(&self, steam_id: u64, app_id: u32) -> Result<Vec<u8>> {
        match self.cached.get(&app_id) {
            Some(ticket) if steam_id == SID => Ok(ticket.clone()),
            _ => bail!("no unexpired cached ownership ticket for app {app_id}"),
        }
    }

    async fn encrypted_app_ticket(
        &self,
        app_id: u32,
        userdata: Vec<u8>,
    ) -> Result<EncryptedTicketOutcome> {
        Ok(match app_id {
            480 => EncryptedTicketOutcome::Issued(userdata),
            70 => EncryptedTicketOutcome::Refused(15),
            _ => bail!("Steam timed out"),
        })
    }

    async fn dlcs(&self, app_id: u32) -> Result<Vec<DlcInfo>> {
        if app_id != 480 {
            bail!("no appinfo for {app_id}");
        }
        Ok(vec![
            DlcInfo {
                app_id: 1001,
                name: Some("Soundtrack".into()),
                content_ready: true,
                disabled: false,
            },
            DlcInfo {
                app_id: 1002,
                name: None,
                content_ready: false,
                disabled: true,
            },
        ])
    }

    async fn app_info(&self, app_id: u32) -> Result<AppInstallInfo> {
        Ok(match app_id {
            480 => AppInstallInfo {
                build_id: 77,
                install_dir: Some("/games/Spacewar".into()),
                language: Some("german".into()),
            },
            _ => AppInstallInfo {
                language: Some("english".into()),
                ..AppInstallInfo::default()
            },
        })
    }

    async fn session_info(&self) -> Result<(u32, String)> {
        Ok((1_800_000_000, "DE".into()))
    }

    async fn friends(&self) -> Result<FriendsSnapshot> {
        Ok(FriendsSnapshot {
            name: "Me".into(),
            state: 1,
            friends: vec![Friend {
                steam_id: SID + 1,
                relationship: 3,
                persona_name: Some("Pal".into()),
                persona_state: Some(3),
                game_app_id: Some(480),
                game_name: None,
                avatar_hash: Some("ab01".into()),
            }],
        })
    }

    async fn set_rich_presence(&self, app_id: u32, pairs: Vec<(String, String)>) -> Result<()> {
        self.presence.lock().unwrap().push((app_id, pairs));
        Ok(())
    }

    async fn friend_rich_presence(&self, steam_id: u64) -> Result<Vec<(String, String)>> {
        if steam_id == SID + 1 {
            Ok(vec![("status".into(), "Lobby".into())])
        } else {
            bail!("Steam sent no rich presence for {steam_id}")
        }
    }

    async fn avatar(&self, steam_id: u64, size: AvatarSize) -> Result<Arc<Avatar>> {
        if steam_id != SID + 1 {
            bail!("Steam sent no avatar for {steam_id}");
        }
        let side = if size == AvatarSize::Small { 2 } else { 3 };
        Ok(Arc::new(Avatar {
            width: side,
            height: side,
            rgba: vec![7; (side * side * 4) as usize],
        }))
    }

    async fn open_url(&self, url: String) -> Result<bool> {
        self.opened.lock().unwrap().push(url);
        Ok(true)
    }

    async fn cloud_info(&self, steam_id: u64, app_id: u32) -> Result<RemoteStorageInfo> {
        if app_id == 70 {
            bail!("no home directory");
        }
        Ok(RemoteStorageInfo {
            enabled: true,
            writable: false,
            remote_dir: format!("/cloud/{steam_id}/{app_id}/remote").into(),
        })
    }

    async fn create_lobby(&self, _app: u32, lobby_type: i32, max: i32) -> Result<(i32, u64)> {
        self.lobby_calls
            .lock()
            .unwrap()
            .push(format!("create {lobby_type} {max}"));
        Ok((1, LOBBY))
    }

    async fn join_lobby(&self, _app: u32, lobby: u64) -> Result<JoinedLobby> {
        if lobby != LOBBY {
            bail!("no such lobby");
        }
        Ok(JoinedLobby {
            enter_response: 1,
            state: sample_lobby(),
        })
    }

    async fn leave_lobby(&self, _app: u32, lobby: u64) -> Result<i32> {
        self.left.lock().unwrap().push(lobby);
        Ok(1)
    }

    async fn lobby_list(
        &self,
        _app: u32,
        count: i32,
        filters: Vec<LobbyFilter>,
    ) -> Result<(i32, Vec<LobbyState>)> {
        self.lobby_calls
            .lock()
            .unwrap()
            .push(format!("list {count} {filters:?}"));
        Ok((1, vec![sample_lobby()]))
    }

    async fn set_lobby_data(&self, _app: u32, update: LobbyUpdate) -> Result<()> {
        self.lobby_calls.lock().unwrap().push(format!("{update:?}"));
        Ok(())
    }

    async fn send_lobby_chat(&self, _app: u32, lobby: u64, message: Vec<u8>) -> Result<()> {
        self.lobby_calls
            .lock()
            .unwrap()
            .push(format!("chat {lobby} {message:?}"));
        Ok(())
    }

    async fn request_lobby_data(&self, _app: u32, lobby: u64) -> Result<()> {
        self.lobby_calls
            .lock()
            .unwrap()
            .push(format!("request {lobby}"));
        Ok(())
    }

    async fn lobby_events(&self, _app: u32) -> Option<mpsc::UnboundedReceiver<LobbyEvent>> {
        let (tx, rx) = mpsc::unbounded_channel();
        *self.lobby_tx.lock().unwrap() = Some(tx);
        Some(rx)
    }
    async fn ugc_query(&self, app_id: u32, query: UgcQuery) -> Result<UgcPage> {
        if app_id != 480 {
            bail!("no workshop for {app_id}");
        }
        self.queries.lock().unwrap().push(query);
        Ok(sample_page())
    }

    async fn ugc_subscribe(&self, _app_id: u32, id: u64, subscribe: bool) -> Result<()> {
        let mut subs = self.subscribed.lock().unwrap();
        subs.retain(|s| *s != id);
        if subscribe {
            subs.push(id);
        }
        Ok(())
    }

    async fn ugc_subscribed(&self, _app_id: u32) -> Result<Vec<u64>> {
        Ok(self.subscribed.lock().unwrap().clone())
    }

    async fn ugc_state(&self, _app_id: u32, id: u64) -> Result<UgcInstall> {
        Ok(if id == 42 {
            UgcInstall {
                installed: true,
                needs_update: true,
                size: 10,
                timestamp: 20,
                folder: "/w/42".into(),
            }
        } else {
            UgcInstall::default()
        })
    }

    async fn ugc_download(
        &self,
        _app_id: u32,
        id: u64,
        progress: Arc<std::sync::RwLock<DownloadState>>,
    ) -> Result<()> {
        progress.write().unwrap().total_bytes = 100;
        if id == 13 {
            bail!("no content");
        }
        Ok(())
    }

    async fn user_stats(&self, app_id: u32, steam_id: Option<u64>) -> Result<UserStats> {
        if app_id != 480 {
            bail!("no stats for {app_id}");
        }
        Ok(sample_stats(steam_id.unwrap_or(SID)))
    }

    async fn stats_writes(&self) -> bool {
        self.writes
    }

    async fn store_stats(
        &self,
        app_id: u32,
        crc: u32,
        reset: bool,
        changes: Vec<(u32, u32)>,
    ) -> Result<StoreOutcome> {
        let reverted = changes
            .iter()
            .filter(|(id, _)| *id == 99)
            .map(|(id, _)| (*id, 0))
            .collect();
        self.stored
            .lock()
            .unwrap()
            .push((app_id, crc, reset, changes));
        Ok(StoreOutcome {
            eresult: 1,
            crc: crc + 1,
            reverted,
        })
    }

    async fn offline_app_info(&self, app_id: u32) -> Result<AppInstallInfo> {
        Ok(AppInstallInfo {
            build_id: app_id + 1,
            install_dir: Some("/games/offline".into()),
            language: Some("french".into()),
        })
    }

    async fn offline_dlcs(&self, app_id: u32) -> Result<Option<Vec<DlcInfo>>> {
        Ok((app_id == 480).then(|| {
            vec![DlcInfo {
                app_id: 1001,
                name: Some("Saved".into()),
                content_ready: true,
                disabled: false,
            }]
        }))
    }
}

/// Stats keyed to the asked account.
fn sample_stats(steam_id: u64) -> UserStats {
    use aurelia::steam_client::user_stats::{Achievement, Stat};
    UserStats {
        eresult: 1,
        crc: 0xC0,
        stats: vec![Stat {
            id: 1,
            name: "Kills".into(),
            kind: 1,
            value: (steam_id % 100) as u32,
        }],
        achievements: vec![Achievement {
            block: 4,
            bit: 2,
            name: "ACH_X".into(),
            display_name: "X".into(),
            description: "Do X".into(),
            hidden: true,
            unlock_time: 7,
        }],
    }
}

fn with_writes() -> Arc<Fake> {
    Arc::new(Fake {
        steam_id: Mutex::new(Some(SID)),
        writes: true,
        ..Fake::default()
    })
}

/// Offline daemon with cached tickets.
fn offline(cached: &[(u32, Vec<u8>)]) -> Arc<Fake> {
    Arc::new(Fake {
        offline_id: Some(SID),
        cached: cached.iter().cloned().collect(),
        ..Fake::default()
    })
}

fn sample_item() -> UgcDetails {
    UgcDetails {
        id: 42,
        result: 1,
        title: "Map".into(),
        tags: "Maps".into(),
        file: 999,
        score: 0.5,
        ..UgcDetails::default()
    }
}

fn sample_page() -> UgcPage {
    UgcPage {
        total: 7,
        next_cursor: "next".into(),
        items: vec![sample_item()],
    }
}

/// Query body like the .so sends.
fn ugc_query_body(kind: u8, page: u32, cursor: &str, tags: &[&str], ids: &[u64]) -> Vec<u8> {
    let mut out = vec![kind];
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&page.to_le_bytes());
    put_blob(&mut out, cursor.as_bytes());
    put_blob(&mut out, b"castle");
    out.extend_from_slice(&(tags.len() as u32).to_le_bytes());
    for t in tags {
        put_blob(&mut out, t.as_bytes());
    }
    out.extend_from_slice(&(ids.len() as u32).to_le_bytes());
    for id in ids {
        out.extend_from_slice(&id.to_le_bytes());
    }
    out
}

fn hello(app_id: u32) -> Vec<u8> {
    let mut out = PROTOCOL_VERSION.to_le_bytes().to_vec();
    out.extend_from_slice(&app_id.to_le_bytes());
    out.extend_from_slice(&4242u32.to_le_bytes());
    out
}

fn request(id: u32, command: u8, body: &[u8]) -> Vec<u8> {
    request_for(id, 0, command, body)
}

fn request_for(id: u32, app_id: u32, command: u8, body: &[u8]) -> Vec<u8> {
    let mut out = id.to_le_bytes().to_vec();
    out.extend_from_slice(&app_id.to_le_bytes());
    out.extend_from_slice(&1i32.to_le_bytes());
    out.extend_from_slice(&1i32.to_le_bytes());
    out.push(command);
    out.extend_from_slice(body);
    out
}

struct Game {
    stream: DuplexStream,
    server: tokio::task::JoinHandle<Result<()>>,
}

impl Game {
    /// Connects; returns welcome payload.
    async fn connect(
        backend: Arc<Fake>,
        router: Arc<Router>,
        hello: Vec<u8>,
        peer_pid: Option<u32>,
    ) -> (Self, Vec<u8>) {
        let (mut stream, daemon) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(async move {
            let (mut reader, writer) = proto::split_shared(daemon);
            let (kind, payload) = proto::read_frame(&mut reader).await?.unwrap();
            assert_eq!(kind, proto::C_SW_HELLO);
            serve(reader, writer, &payload, peer_pid, backend, router).await
        });
        proto::write_frame(&mut stream, proto::C_SW_HELLO, &hello)
            .await
            .unwrap();
        let (kind, welcome) = proto::read_frame(&mut stream).await.unwrap().unwrap();
        assert_eq!(kind, proto::C_SW_WELCOME);
        (Self { stream, server }, welcome)
    }

    async fn ask(&mut self, id: u32, command: u8, body: &[u8]) -> (u8, Vec<u8>) {
        self.ask_for(id, 0, command, body).await
    }

    async fn ask_for(&mut self, id: u32, app_id: u32, command: u8, body: &[u8]) -> (u8, Vec<u8>) {
        let req = request_for(id, app_id, command, body);
        proto::write_frame(&mut self.stream, proto::C_SW_REQUEST, &req)
            .await
            .unwrap();
        let (kind, payload) = self.next().await;
        assert_eq!(kind, proto::C_SW_RESPONSE);
        let mut f = Fields::new(&payload);
        assert_eq!(f.u32(), Some(id));
        let code = f.u8().unwrap();
        (code, f.rest().to_vec())
    }

    async fn next(&mut self) -> (u8, Vec<u8>) {
        proto::read_frame(&mut self.stream).await.unwrap().unwrap()
    }

    async fn hang_up(self) {
        drop(self.stream);
        self.server.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn welcome_reports_session_and_starts_launch() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (game, welcome) = Game::connect(fake.clone(), router, hello(480), Some(99)).await;
    assert_eq!(welcome, welcome_payload(welcome::OK, SID, ""));
    // Peer credentials beat hello pid.
    assert_eq!(*fake.started.lock().unwrap(), vec![(480, Some(99))]);
    game.hang_up().await;
    assert_eq!(*fake.stopped.lock().unwrap(), vec![7]);
}

#[tokio::test]
async fn hello_pid_used_without_credentials() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    assert_eq!(*fake.started.lock().unwrap(), vec![(480, Some(4242))]);
    game.hang_up().await;
}

#[tokio::test]
async fn no_session_refuses_without_launch() {
    let fake = Arc::new(Fake::default());
    let router = Arc::new(Router::default());
    let (game, welcome) = Game::connect(fake.clone(), router, hello(480), None).await;
    assert_eq!(welcome[0], welcome::NO_SESSION);
    assert!(fake.started.lock().unwrap().is_empty());
    game.server.await.unwrap().unwrap();
}

#[tokio::test]
async fn bad_hellos_are_refused() {
    let router = Arc::new(Router::default());
    let mut old = hello(480);
    old[0] = 9;
    for bad in [hello(0), old, vec![1, 2]] {
        let fake = Fake::logged_in();
        let (game, welcome) = Game::connect(fake.clone(), router.clone(), bad, None).await;
        assert_eq!(welcome[0], welcome::REFUSED);
        assert!(fake.started.lock().unwrap().is_empty());
        game.server.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn playing_conflict_is_refused() {
    let fake = Arc::new(Fake {
        steam_id: Mutex::new(Some(SID)),
        conflict: true,
        ..Fake::default()
    });
    let router = Arc::new(Router::default());
    let (game, welcome) = Game::connect(fake, router, hello(480), None).await;
    let mut f = Fields::new(&welcome);
    assert_eq!(f.u8(), Some(welcome::REFUSED));
    assert_eq!(f.u64(), Some(0));
    assert_eq!(f.blob(), Some(&b"another client is playing"[..]));
    game.server.await.unwrap().unwrap();
}

#[tokio::test]
async fn status_and_ownership_round_trip() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router, hello(480), None).await;
    let mut expected = vec![1];
    expected.extend_from_slice(&SID.to_le_bytes());
    assert_eq!(game.ask(1, cmd::STATUS, &[]).await, (status::OK, expected));
    assert_eq!(
        game.ask(2, cmd::OWNERSHIP_TICKET, &[]).await,
        (status::OK, vec![0xAA, 0xBB])
    );
    assert_eq!(game.ask(3, 0x7F, &[]).await.0, status::UNSUPPORTED);
    game.hang_up().await;
}

#[tokio::test]
async fn unowned_app_fails() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router, hello(70), None).await;
    let (code, body) = game.ask(1, cmd::OWNERSHIP_TICKET, &[]).await;
    assert_eq!(code, status::FAILED);
    assert_eq!(body, b"app 70 not owned");
    game.hang_up().await;
}

#[tokio::test]
async fn lost_session_fails_closed() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    *fake.steam_id.lock().unwrap() = None;
    let (code, _) = game.ask(1, cmd::OWNERSHIP_TICKET, &[]).await;
    assert_eq!(code, status::NO_SESSION);
    let (code, body) = game.ask(2, cmd::STATUS, &[]).await;
    assert_eq!((code, body[0]), (status::OK, 0));
    game.hang_up().await;
}

#[tokio::test]
async fn auth_ticket_create_push_cancel() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router.clone(), hello(480), None).await;

    let mut body = vec![ticket_kind::SESSION];
    put_blob(&mut body, b"");
    let (code, reply) = game.ask(1, cmd::CREATE_AUTH_TICKET, &body).await;
    assert_eq!(code, status::OK);
    let mut f = Fields::new(&reply);
    assert_eq!(f.u32(), Some(101));
    assert_eq!(f.u32(), Some(0xDEAD_BEEF));
    assert_eq!(f.u8(), Some(1));
    assert_eq!(f.blob(), Some(&[1u8, 2, 3][..]));

    let event = TicketAuthEvent {
        handle: Some(101),
        steam_id: SID,
        owner_steam_id: SID,
        game_id: 480,
        ticket_crc: 0xDEAD_BEEF,
        estate: 3,
        response: 0,
    };
    let stray = TicketAuthEvent {
        handle: Some(555),
        ..event.clone()
    };
    fake.events.lock().unwrap().extend([stray, event.clone()]);
    router.dispatch(fake.ticket_events().await);
    let (kind, payload) = game.next().await;
    assert_eq!(kind, proto::C_SW_PUSH);
    assert_eq!(payload, ticket_auth_payload(101, &event));

    // Foreign handles are not cancelled.
    let (code, found) = game
        .ask(2, cmd::CANCEL_AUTH_TICKET, &555u32.to_le_bytes())
        .await;
    assert_eq!((code, found), (status::OK, vec![0]));
    let (code, found) = game
        .ask(3, cmd::CANCEL_AUTH_TICKET, &101u32.to_le_bytes())
        .await;
    assert_eq!((code, found), (status::OK, vec![1]));
    assert!(router.is_empty());
    assert_eq!(*fake.cancelled.lock().unwrap(), vec![101]);
    game.hang_up().await;
}

#[tokio::test]
async fn web_api_identity_and_disconnect_cleanup() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router.clone(), hello(480), None).await;
    let mut body = vec![ticket_kind::WEB_API];
    put_blob(&mut body, b"svc");
    let (code, reply) = game.ask(1, cmd::CREATE_AUTH_TICKET, &body).await;
    assert_eq!((code, &reply[..4]), (status::OK, &200u32.to_le_bytes()[..]));
    let (code, _) = game.ask(2, cmd::CREATE_AUTH_TICKET, &[9]).await;
    assert_eq!(code, status::MALFORMED);
    assert!(!router.is_empty());
    game.hang_up().await;
    assert!(router.is_empty());
    assert_eq!(*fake.cancelled.lock().unwrap(), vec![200]);
    assert_eq!(*fake.stopped.lock().unwrap(), vec![7]);
}

#[tokio::test]
async fn malformed_request_keeps_connection() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router, hello(480), None).await;
    proto::write_frame(&mut game.stream, proto::C_SW_REQUEST, &[5, 0, 0, 0])
        .await
        .unwrap();
    let (kind, payload) = game.next().await;
    assert_eq!(kind, proto::C_SW_RESPONSE);
    assert_eq!(&payload[..5], &[5, 0, 0, 0, status::MALFORMED]);
    assert_eq!(game.ask(6, cmd::STATUS, &[]).await.0, status::OK);
    game.hang_up().await;
}

#[tokio::test]
async fn ownership_for_other_apps_uses_request_app() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router, hello(70), None).await;
    let (code, body) = game.ask_for(1, 480, cmd::OWNERSHIP_TICKET, &[]).await;
    assert_eq!((code, body), (status::OK, vec![0xAA, 0xBB]));
    assert_eq!(
        game.ask(2, cmd::OWNERSHIP_TICKET, &[]).await.0,
        status::FAILED
    );
    game.hang_up().await;
}

#[tokio::test]
async fn encrypted_ticket_issued_refused_failed() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router, hello(480), None).await;
    let (code, body) = game.ask(1, cmd::ENCRYPTED_APP_TICKET, b"data").await;
    assert_eq!(code, status::OK);
    let mut f = Fields::new(&body);
    assert_eq!(f.i32(), Some(1));
    assert_eq!(f.blob(), Some(&b"data"[..]));
    let (code, body) = game.ask_for(2, 70, cmd::ENCRYPTED_APP_TICKET, &[]).await;
    assert_eq!(code, status::OK);
    let mut f = Fields::new(&body);
    assert_eq!((f.i32(), f.blob()), (Some(15), Some(&[][..])));
    let (code, body) = game.ask_for(3, 9, cmd::ENCRYPTED_APP_TICKET, &[]).await;
    assert_eq!((code, body), (status::FAILED, b"Steam timed out".to_vec()));
    game.hang_up().await;
}

#[tokio::test]
async fn dlc_app_and_session_info() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router, hello(480), None).await;
    let (code, body) = game.ask(1, cmd::DLC_LIST, &[]).await;
    assert_eq!(code, status::OK);
    let mut f = Fields::new(&body);
    assert_eq!(f.u32(), Some(2));
    let flags = dlc_flag::AVAILABLE | dlc_flag::CONTENT_READY;
    assert_eq!(
        (f.u32(), f.u8(), f.blob()),
        (Some(1001), Some(flags), Some(&b"Soundtrack"[..]))
    );
    let flags = dlc_flag::DISABLED;
    assert_eq!(
        (f.u32(), f.u8(), f.blob()),
        (Some(1002), Some(flags), Some(&[][..]))
    );
    assert!(f.rest().is_empty());
    assert_eq!(
        game.ask_for(2, 70, cmd::DLC_LIST, &[]).await.0,
        status::FAILED
    );

    let (code, body) = game.ask(3, cmd::APP_INFO, &[]).await;
    assert_eq!(code, status::OK);
    let mut f = Fields::new(&body);
    assert_eq!(f.u32(), Some(77));
    assert_eq!(f.blob(), Some(&b"/games/Spacewar"[..]));
    assert_eq!(f.blob(), Some(&b"german"[..]));
    let (_, body) = game.ask_for(4, 70, cmd::APP_INFO, &[]).await;
    let mut f = Fields::new(&body);
    assert_eq!((f.u32(), f.blob()), (Some(0), Some(&[][..])));

    let (code, body) = game.ask(5, cmd::SESSION_INFO, &[]).await;
    assert_eq!(
        (code, body),
        (status::OK, session_info_body(1_800_000_000, "DE"))
    );
    game.hang_up().await;
}

#[tokio::test]
async fn licensing_needs_a_session() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    *fake.steam_id.lock().unwrap() = None;
    for (id, command) in [
        cmd::ENCRYPTED_APP_TICKET,
        cmd::DLC_LIST,
        cmd::APP_INFO,
        cmd::SESSION_INFO,
        cmd::CLOUD_INFO,
    ]
    .into_iter()
    .enumerate()
    {
        let (code, _) = game.ask(id as u32, command, &[]).await;
        assert_eq!(code, status::NO_SESSION, "command {command}");
    }
    game.hang_up().await;
}

/// Same bytes the .so tests use.
#[test]
fn wire_golden_bytes() {
    assert_eq!(
        Hello::parse(&[1, 0, 0, 0, 0xE0, 1, 0, 0, 0x39, 0x30, 0, 0]),
        Some(Hello {
            version: 1,
            app_id: 480,
            pid: 12345
        })
    );
    let req = [9, 0, 0, 0, 0xE0, 1, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 2, 0xAB];
    assert_eq!(
        Request::parse(&req),
        Some(Request {
            id: 9,
            app_id: 480,
            pipe: 2,
            user: 3,
            command: cmd::OWNERSHIP_TICKET,
            body: vec![0xAB],
        })
    );
    assert_eq!(
        welcome_payload(welcome::OK, 0x0110_0001_0000_0002, "hi"),
        [0, 2, 0, 0, 0, 1, 0, 0x10, 1, 2, 0, 0, 0, b'h', b'i']
    );
    assert_eq!(
        response_payload(9, status::FAILED, b"x"),
        [9, 0, 0, 0, 1, b'x']
    );
    let ticket = AuthSessionTicket {
        handle: 5,
        app_id: 480,
        crc: 6,
        ticket: vec![7],
        acknowledged: false,
    };
    assert_eq!(
        auth_ticket_body(&ticket),
        [5, 0, 0, 0, 6, 0, 0, 0, 0, 1, 0, 0, 0, 7]
    );
    assert_eq!(
        encrypted_ticket_body(&EncryptedTicketOutcome::Issued(vec![7])),
        [1, 0, 0, 0, 1, 0, 0, 0, 7]
    );
    let dlc = DlcInfo {
        app_id: 1001,
        name: Some("A".into()),
        content_ready: true,
        disabled: false,
    };
    assert_eq!(
        dlc_list_body(&[dlc]),
        [1, 0, 0, 0, 0xE9, 3, 0, 0, 3, 1, 0, 0, 0, b'A']
    );
    let info = AppInstallInfo {
        build_id: 5,
        install_dir: Some("/g".into()),
        language: Some("de".into()),
    };
    assert_eq!(
        app_info_body(&info),
        [5, 0, 0, 0, 2, 0, 0, 0, b'/', b'g', 2, 0, 0, 0, b'd', b'e']
    );
    assert_eq!(
        session_info_body(0x0102_0304, "US"),
        [4, 3, 2, 1, 2, 0, 0, 0, b'U', b'S']
    );
}

#[tokio::test]
async fn offline_cached_ticket_admits_game() {
    let fake = offline(&[(480, vec![0xCC])]);
    let router = Arc::new(Router::default());
    let (mut game, welcome) = Game::connect(fake.clone(), router, hello(480), None).await;
    assert_eq!(welcome, welcome_payload(welcome::OK, SID, OFFLINE_WELCOME));
    // Nothing reported to Steam.
    assert!(fake.started.lock().unwrap().is_empty());
    assert_eq!(
        game.ask(1, cmd::OWNERSHIP_TICKET, &[]).await,
        (status::OK, vec![0xCC])
    );
    let (code, body) = game.ask(2, cmd::STATUS, &[]).await;
    assert_eq!((code, body[0]), (status::OK, 0));
    game.hang_up().await;
    assert!(fake.stopped.lock().unwrap().is_empty());
}

#[tokio::test]
async fn offline_fails_closed_for_everything_else() {
    let fake = offline(&[(480, vec![0xCC])]);
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router.clone(), hello(480), None).await;
    let mut body = vec![ticket_kind::SESSION];
    put_blob(&mut body, b"");
    assert_eq!(
        game.ask(1, cmd::CREATE_AUTH_TICKET, &body).await,
        (status::NO_SESSION, b"offline".to_vec())
    );
    let (code, _) = game
        .ask(2, cmd::CANCEL_AUTH_TICKET, &1u32.to_le_bytes())
        .await;
    assert_eq!(code, status::NO_SESSION);
    assert_eq!(game.ask(3, 0x7F, &[]).await.0, status::UNSUPPORTED);
    assert_eq!(
        game.ask(4, cmd::CLOUD_INFO, &[]).await.0,
        status::NO_SESSION
    );
    assert!(router.is_empty());
    game.hang_up().await;
    assert!(fake.cancelled.lock().unwrap().is_empty());
}

#[tokio::test]
async fn offline_without_cached_ticket_is_refused() {
    let fake = offline(&[(70, vec![1])]);
    let router = Arc::new(Router::default());
    let (game, welcome) = Game::connect(fake.clone(), router, hello(480), None).await;
    assert_eq!(welcome[0], welcome::NO_SESSION);
    assert!(fake.started.lock().unwrap().is_empty());
    game.server.await.unwrap().unwrap();
}

#[tokio::test]
async fn offline_other_app_needs_its_own_ticket() {
    let fake = offline(&[(480, vec![0xCC])]);
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router, hello(480), None).await;
    let mut req = request(1, cmd::OWNERSHIP_TICKET, &[]);
    req[4..8].copy_from_slice(&70u32.to_le_bytes());
    proto::write_frame(&mut game.stream, proto::C_SW_REQUEST, &req)
        .await
        .unwrap();
    let (_, payload) = game.next().await;
    assert_eq!(payload[4], status::FAILED);
    game.hang_up().await;
}

#[tokio::test]
async fn offline_connection_stays_cache_only() {
    let fake = offline(&[(480, vec![0xCC])]);
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    *fake.steam_id.lock().unwrap() = Some(SID);
    assert_eq!(
        game.ask(1, cmd::OWNERSHIP_TICKET, &[]).await,
        (status::OK, vec![0xCC])
    );
    game.hang_up().await;
    assert!(fake.started.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cloud_info_round_trip() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router, hello(480), None).await;
    let (code, body) = game.ask(1, cmd::CLOUD_INFO, &[]).await;
    assert_eq!(code, status::OK);
    let mut f = Fields::new(&body);
    assert_eq!(f.u8(), Some(cloud_flag::ENABLED));
    let dir = format!("/cloud/{SID}/480/remote");
    assert_eq!(f.blob(), Some(dir.as_bytes()));
    game.hang_up().await;

    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router, hello(70), None).await;
    assert_eq!(game.ask(1, cmd::CLOUD_INFO, &[]).await.0, status::FAILED);
    game.hang_up().await;
}

const LOBBY: u64 = 109_775_240_000_000_001;

fn sample_lobby() -> LobbyState {
    LobbyState {
        lobby: LOBBY,
        owner: SID,
        max_members: 4,
        lobby_type: 2,
        flags: 0,
        num_members: 1,
        metadata: vec![("mode".into(), "ctf".into())],
        members: vec![aurelia::steam_client::lobbies::LobbyMember {
            steam_id: SID,
            name: "Owner".into(),
            metadata: Vec::new(),
        }],
    }
}

#[tokio::test]
async fn lobby_create_join_list_and_leave_on_disconnect() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    let mut body = 2i32.to_le_bytes().to_vec();
    body.extend_from_slice(&4i32.to_le_bytes());
    let (code, reply) = game.ask(1, lobby::cmd::CREATE, &body).await;
    assert_eq!(code, status::OK);
    assert_eq!(reply, lobby::created_body(1, LOBBY));

    let (code, reply) = game.ask(2, lobby::cmd::JOIN, &LOBBY.to_le_bytes()).await;
    assert_eq!(code, status::OK);
    assert_eq!(reply, lobby::joined_body(1, &sample_lobby()));
    let (code, _) = game.ask(3, lobby::cmd::JOIN, &5u64.to_le_bytes()).await;
    assert_eq!(code, status::FAILED);

    let mut list = 10i32.to_le_bytes().to_vec();
    list.extend_from_slice(&1u32.to_le_bytes());
    put_blob(&mut list, b"mode");
    put_blob(&mut list, b"ctf");
    list.extend_from_slice(&0i32.to_le_bytes());
    list.extend_from_slice(&0i32.to_le_bytes());
    let (code, reply) = game.ask(4, lobby::cmd::LIST, &list).await;
    assert_eq!(code, status::OK);
    assert_eq!(reply, lobby::list_body(1, &[sample_lobby()]));
    assert_eq!(
        game.ask(5, lobby::cmd::LIST, &[1]).await.0,
        status::MALFORMED
    );
    game.hang_up().await;
    assert_eq!(*fake.left.lock().unwrap(), vec![LOBBY]);
    let calls = fake.lobby_calls.lock().unwrap().join("|");
    assert!(calls.contains("create 2 4"), "{calls}");
    assert!(calls.contains("list 10"), "{calls}");
}

#[tokio::test]
async fn lobby_data_chat_and_pushes() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    let mut update = LOBBY.to_le_bytes().to_vec();
    update.extend_from_slice(&SID.to_le_bytes());
    for v in [4i32, 2, 0] {
        update.extend_from_slice(&v.to_le_bytes());
    }
    lobby::put_metadata(&mut update, &vec![("ready".into(), "1".into())]);
    assert_eq!(
        game.ask(1, lobby::cmd::SET_DATA, &update).await.0,
        status::OK
    );
    let mut chat = LOBBY.to_le_bytes().to_vec();
    chat.extend_from_slice(b"hi");
    assert_eq!(game.ask(2, lobby::cmd::CHAT, &chat).await.0, status::OK);
    let req = game
        .ask(3, lobby::cmd::REQUEST_DATA, &LOBBY.to_le_bytes())
        .await;
    assert_eq!(req.0, status::OK);
    let calls = fake.lobby_calls.lock().unwrap().join("|");
    assert!(calls.contains("member: 76561197960287930"), "{calls}");
    assert!(
        calls.contains("chat 109775240000000001 [104, 105]"),
        "{calls}"
    );

    let event = LobbyEvent::Chat {
        lobby: LOBBY,
        sender: SID,
        message: b"yo".to_vec(),
    };
    let tx = fake.lobby_tx.lock().unwrap().clone().unwrap();
    tx.send(event.clone()).unwrap();
    let (kind, payload) = game.next().await;
    assert_eq!(kind, proto::C_SW_PUSH);
    assert_eq!(payload, lobby::event_payload(&event));
    game.hang_up().await;
    assert!(fake.left.lock().unwrap().is_empty());
}

#[tokio::test]
async fn lobbies_need_a_session() {
    let fake = offline(&[(480, vec![0xCC])]);
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    let (code, _) = game.ask(1, lobby::cmd::JOIN, &LOBBY.to_le_bytes()).await;
    assert_eq!(code, status::NO_SESSION);
    assert!(fake.lobby_tx.lock().unwrap().is_none());
    game.hang_up().await;
}

#[test]
fn cloud_info_golden_bytes() {
    let info = RemoteStorageInfo {
        enabled: true,
        writable: true,
        remote_dir: "/r".into(),
    };
    assert_eq!(cloud_info_body(&info), [3, 2, 0, 0, 0, b'/', b'r']);
}

#[test]
fn lobby_push_golden_bytes() {
    let left = LobbyEvent::Left { lobby: 1, user: 2 };
    assert_eq!(
        lobby::event_payload(&left),
        [62, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]
    );
    let state = LobbyState {
        lobby: 1,
        owner: 2,
        max_members: 3,
        lobby_type: 2,
        flags: 0,
        num_members: 0,
        metadata: vec![("k".into(), "v".into())],
        members: Vec::new(),
    };
    let mut out = Vec::new();
    lobby::put_state(&mut out, &state);
    assert_eq!(
        out,
        [
            1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, b'k', 1, 0, 0, 0, b'v', 0, 0, 0, 0
        ]
    );
}

#[tokio::test]
async fn user_stats_flag_writes_only_for_self() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router.clone(), hello(480), None).await;
    let (code, body) = game.ask(1, cmd::USER_STATS, &0u64.to_le_bytes()).await;
    assert_eq!(
        (code, body),
        (status::OK, user_stats_body(&sample_stats(SID), false))
    );
    let other = 76_561_197_960_287_999u64;
    let (code, body) = game.ask(2, cmd::USER_STATS, &other.to_le_bytes()).await;
    assert_eq!(
        (code, body),
        (status::OK, user_stats_body(&sample_stats(other), false))
    );
    assert_eq!(
        game.ask(3, cmd::USER_STATS, &[1]).await.0,
        status::MALFORMED
    );
    let (code, _) = game
        .ask_for(4, 70, cmd::USER_STATS, &0u64.to_le_bytes())
        .await;
    assert_eq!(code, status::FAILED);
    game.hang_up().await;

    let (mut game, _) = Game::connect(with_writes(), router, hello(480), None).await;
    let (_, body) = game.ask(1, cmd::USER_STATS, &0u64.to_le_bytes()).await;
    assert_eq!(body[8], stats_flag::WRITES);
    let (_, body) = game.ask(2, cmd::USER_STATS, &other.to_le_bytes()).await;
    assert_eq!(body[8], 0, "never for other users");
    game.hang_up().await;
}

fn store_body(crc: u32, reset: bool, changes: &[(u32, u32)]) -> Vec<u8> {
    let mut out = crc.to_le_bytes().to_vec();
    out.push(u8::from(reset));
    out.extend_from_slice(&(changes.len() as u32).to_le_bytes());
    for (id, value) in changes {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn kv_body(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut out = (pairs.len() as u32).to_le_bytes().to_vec();
    for (k, v) in pairs {
        put_blob(&mut out, k.as_bytes());
        put_blob(&mut out, v.as_bytes());
    }
    out
}

#[tokio::test]
async fn store_stats_needs_the_switch() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router.clone(), hello(480), None).await;
    let body = store_body(0xC0, false, &[(1, 5)]);
    let (code, reply) = game.ask(1, cmd::STORE_STATS, &body).await;
    assert_eq!(
        (code, reply),
        (status::FAILED, b"stats writes are off".to_vec())
    );
    assert!(fake.stored.lock().unwrap().is_empty());
    game.hang_up().await;

    let fake = with_writes();
    let (mut game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    let body = store_body(0xC0, true, &[(1, 5), (99, 3)]);
    let (code, reply) = game.ask(1, cmd::STORE_STATS, &body).await;
    assert_eq!(code, status::OK);
    let mut f = Fields::new(&reply);
    assert_eq!((f.i32(), f.u32(), f.u32()), (Some(1), Some(0xC1), Some(1)));
    assert_eq!((f.u32(), f.u32()), (Some(99), Some(0)));
    assert_eq!(
        *fake.stored.lock().unwrap(),
        vec![(480, 0xC0, true, vec![(1, 5), (99, 3)])]
    );
    let mut bad = store_body(0, false, &[(1, 5)]);
    bad.push(0);
    assert_eq!(
        game.ask(2, cmd::STORE_STATS, &bad).await.0,
        status::MALFORMED
    );
    let huge = [0, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF];
    assert_eq!(
        game.ask(3, cmd::STORE_STATS, &huge).await.0,
        status::MALFORMED
    );
    game.hang_up().await;
}

#[tokio::test]
async fn friends_snapshot_on_the_wire() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router, hello(480), None).await;
    let (code, body) = game.ask(1, cmd::FRIENDS, &[]).await;
    assert_eq!(code, status::OK);
    let mut f = Fields::new(&body);
    assert_eq!(f.blob(), Some(&b"Me"[..]));
    assert_eq!(f.u32(), Some(1));
    assert_eq!(f.u32(), Some(1));
    assert_eq!(f.u64(), Some(SID + 1));
    assert_eq!((f.u32(), f.u32(), f.u32()), (Some(3), Some(3), Some(480)));
    assert_eq!(f.blob(), Some(&b"Pal"[..]));
    assert!(f.rest().is_empty());
    game.hang_up().await;
}

#[tokio::test]
async fn rich_presence_set_for_connected_app_only() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    let body = kv_body(&[("status", "In menu"), ("steam_display", "#Menu")]);
    let mut req = request(1, cmd::SET_RICH_PRESENCE, &body);
    req[4..8].copy_from_slice(&70u32.to_le_bytes());
    proto::write_frame(&mut game.stream, proto::C_SW_REQUEST, &req)
        .await
        .unwrap();
    let (_, payload) = game.next().await;
    assert_eq!(payload[4], status::OK);
    let set = fake.presence.lock().unwrap().clone();
    assert_eq!(set.len(), 1);
    assert_eq!(set[0].0, 480, "other apps are not ours");
    assert_eq!(
        set[0].1[1],
        ("steam_display".to_string(), "#Menu".to_string())
    );
    game.hang_up().await;
}

#[tokio::test]
async fn rich_presence_limits_are_enforced() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    let long_key = "k".repeat(65);
    let long_value = "v".repeat(257);
    let many: Vec<(String, String)> = (0..31).map(|i| (format!("k{i}"), "v".into())).collect();
    let many: Vec<(&str, &str)> = many.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let bad = [
        kv_body(&[(&long_key, "v")]),
        kv_body(&[("k", &long_value)]),
        kv_body(&[("", "v")]),
        kv_body(&[("a", "1"), ("a", "2")]),
        kv_body(&many),
        vec![1, 0, 0, 0],
    ];
    for (i, body) in bad.iter().enumerate() {
        let (code, _) = game.ask(i as u32 + 1, cmd::SET_RICH_PRESENCE, body).await;
        assert_eq!(code, status::MALFORMED, "case {i}");
    }
    assert_eq!(
        game.ask(9, cmd::SET_RICH_PRESENCE, &kv_body(&[])).await.0,
        status::OK,
        "clearing is allowed"
    );
    assert_eq!(fake.presence.lock().unwrap().len(), 1);
    game.hang_up().await;
}

#[tokio::test]
async fn friend_rich_presence_round_trip() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router, hello(480), None).await;
    let (code, body) = game
        .ask(1, cmd::FRIEND_RICH_PRESENCE, &(SID + 1).to_le_bytes())
        .await;
    assert_eq!((code, body), (status::OK, kv_body(&[("status", "Lobby")])));
    let (code, _) = game
        .ask(2, cmd::FRIEND_RICH_PRESENCE, &(SID + 9).to_le_bytes())
        .await;
    assert_eq!(code, status::FAILED);
    assert_eq!(
        game.ask(3, cmd::FRIEND_RICH_PRESENCE, &[1]).await.0,
        status::MALFORMED
    );
    game.hang_up().await;
}

#[tokio::test]
async fn stats_need_a_session() {
    let fake = with_writes();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    *fake.steam_id.lock().unwrap() = None;
    let (code, _) = game.ask(1, cmd::USER_STATS, &0u64.to_le_bytes()).await;
    assert_eq!(code, status::NO_SESSION);
    let (code, _) = game
        .ask(2, cmd::STORE_STATS, &store_body(0, false, &[]))
        .await;
    assert_eq!(code, status::NO_SESSION);
    assert!(fake.stored.lock().unwrap().is_empty());
    game.hang_up().await;
}

/// Same bytes the .so tests use.
#[test]
fn stats_golden_bytes() {
    use aurelia::steam_client::user_stats::{Achievement, Stat};
    let stats = UserStats {
        eresult: 1,
        crc: 2,
        stats: vec![Stat {
            id: 3,
            name: "K".into(),
            kind: 2,
            value: 4,
        }],
        achievements: vec![Achievement {
            block: 5,
            bit: 6,
            name: "A".into(),
            display_name: "B".into(),
            description: "C".into(),
            hidden: true,
            unlock_time: 7,
        }],
    };
    assert_eq!(
        user_stats_body(&stats, true),
        [
            1, 0, 0, 0, 2, 0, 0, 0, 1, //
            1, 0, 0, 0, 3, 0, 0, 0, 2, 4, 0, 0, 0, 1, 0, 0, 0, b'K', //
            1, 0, 0, 0, 5, 0, 0, 0, 6, 7, 0, 0, 0, 1, //
            1, 0, 0, 0, b'A', 1, 0, 0, 0, b'B', 1, 0, 0, 0, b'C',
        ]
    );
    assert_eq!(
        parse_store_request(&[9, 0, 0, 0, 1, 1, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0]),
        Some((9, true, vec![(3, 4)]))
    );
    let outcome = StoreOutcome {
        eresult: 1,
        crc: 2,
        reverted: vec![(3, 4)],
    };
    assert_eq!(
        store_outcome_body(&outcome),
        [1, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0]
    );
}

#[tokio::test]
async fn open_url_accepts_web_pages_only() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    let url = |u: &str| {
        let mut out = Vec::new();
        put_blob(&mut out, u.as_bytes());
        out
    };
    let page = "https://store.steampowered.com/app/480";
    assert_eq!(
        game.ask(1, cmd::OPEN_URL, &url(page)).await,
        (status::OK, vec![1])
    );
    for bad in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "https://a b",
        "steam://run/1",
    ] {
        assert_eq!(
            game.ask(2, cmd::OPEN_URL, &url(bad)).await.0,
            status::MALFORMED,
            "{bad}"
        );
    }
    assert_eq!(*fake.opened.lock().unwrap(), vec![page.to_string()]);
    game.hang_up().await;
}

#[tokio::test]
async fn ugc_queries_parse_and_answer() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    let bodies = [
        ugc_query_body(ugc_kind::ALL_PAGE, 2, "", &["Maps"], &[]),
        ugc_query_body(ugc_kind::ALL_CURSOR, 0, "abc", &[], &[]),
        ugc_query_body(ugc_kind::DETAILS, 0, "", &[], &[42, 43]),
        ugc_query_body(ugc_kind::SUBSCRIBED, 3, "", &[], &[]),
    ];
    for (i, body) in bodies.iter().enumerate() {
        let (code, reply) = game.ask(i as u32, cmd::UGC_QUERY, body).await;
        assert_eq!((code, reply), (status::OK, ugc_page_body(&sample_page())));
    }
    assert_eq!(
        *fake.queries.lock().unwrap(),
        vec![
            UgcQuery::All {
                query_type: 3,
                page: 2,
                cursor: None,
                search: "castle".into(),
                tags: vec!["Maps".into()],
            },
            UgcQuery::All {
                query_type: 3,
                page: 0,
                cursor: Some("abc".into()),
                search: "castle".into(),
                tags: Vec::new(),
            },
            UgcQuery::Details(vec![42, 43]),
            UgcQuery::Subscribed { page: 3 },
        ]
    );
    let mut bad = ugc_query_body(9, 0, "", &[], &[]);
    assert_eq!(game.ask(9, cmd::UGC_QUERY, &bad).await.0, status::MALFORMED);
    bad = ugc_query_body(ugc_kind::DETAILS, 0, "", &[], &[]);
    bad.truncate(bad.len() - 4);
    bad.extend_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        game.ask(10, cmd::UGC_QUERY, &bad).await.0,
        status::MALFORMED
    );
    let body = ugc_query_body(ugc_kind::ALL_PAGE, 1, "", &[], &[]);
    assert_eq!(
        game.ask_for(11, 70, cmd::UGC_QUERY, &body).await.0,
        status::FAILED
    );
    game.hang_up().await;
}

#[tokio::test]
async fn offline_friends_fail_closed() {
    let fake = offline(&[(480, vec![0xCC])]);
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    for command in [
        cmd::FRIENDS,
        cmd::SET_RICH_PRESENCE,
        cmd::FRIEND_RICH_PRESENCE,
        cmd::OPEN_URL,
    ] {
        assert_eq!(game.ask(1, command, &[]).await.0, status::NO_SESSION);
    }
    assert!(fake.presence.lock().unwrap().is_empty());
    game.hang_up().await;
}

fn purchase(order_id: u64) -> MicroTxnRequest {
    MicroTxnRequest {
        app_id: 480,
        order_id,
        trans_id: order_id + 7,
    }
}

#[test]
fn microtxn_payloads_on_the_wire() {
    let request = purchase(0x0102_0304_0506_0708);
    let mut f_bytes = microtxn_pending_payload(&request);
    assert_eq!(f_bytes.remove(0), push::MICROTXN_PENDING);
    let mut f = Fields::new(&f_bytes);
    assert_eq!(f.u32(), Some(480));
    assert_eq!(f.u64(), Some(0x0102_0304_0506_0708));
    assert_eq!(f.blob(), Some(request.approval_url().as_bytes()));
    assert_eq!(
        microtxn_result_payload(&request, true),
        [81, 0xE0, 1, 0, 0, 8, 7, 6, 5, 4, 3, 2, 1, 1]
    );
}

#[tokio::test]
async fn purchase_decisions_reach_the_game() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router.clone(), hello(480), None).await;
    // Wait for registration.
    let ready = tokio::time::Instant::now() + Duration::from_secs(5);
    while router.push_to_app(480, &[0xFF]) == 0 {
        assert!(tokio::time::Instant::now() < ready, "game not registered");
        tokio::task::yield_now().await;
    }
    let (kind, probe) = game.next().await;
    assert_eq!((kind, probe), (proto::C_SW_PUSH, vec![0xFF]));

    microtxn::record(purchase(70_000_001));
    let decided = microtxn_decide(&router, 70_000_001, true).unwrap();
    assert_eq!(decided.order_id, 70_000_001);
    let (kind, payload) = game.next().await;
    assert_eq!(kind, proto::C_SW_PUSH);
    assert_eq!(
        payload,
        microtxn_result_payload(&purchase(70_000_001), true)
    );
    assert!(
        microtxn_decide(&router, 70_000_001, true).is_err(),
        "taken once"
    );

    game.hang_up().await;
    assert_eq!(router.push_to_app(480, &[0xFF]), 0);
    microtxn::record(purchase(70_000_002));
    assert!(microtxn_decide(&router, 70_000_002, false).is_err());
    assert!(
        microtxn::pending().iter().any(|p| p.order_id == 70_000_002),
        "kept for a retry"
    );
    let _ = microtxn::take(70_000_002);
}

#[tokio::test]
async fn offline_games_get_no_purchases() {
    let fake = offline(&[(480, vec![0xCC])]);
    let router = Arc::new(Router::default());
    let (game, _) = Game::connect(fake, router.clone(), hello(480), None).await;
    tokio::task::yield_now().await;
    assert_eq!(router.push_to_app(480, &[0xFF]), 0);
    game.hang_up().await;
}

#[tokio::test]
async fn offline_serves_local_app_info_and_saved_dlc() {
    let fake = offline(&[(480, vec![0xCC]), (70, vec![0xDD])]);
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router.clone(), hello(480), None).await;
    let info = AppInstallInfo {
        build_id: 481,
        install_dir: Some("/games/offline".into()),
        language: Some("french".into()),
    };
    assert_eq!(
        game.ask(1, cmd::APP_INFO, &[]).await,
        (status::OK, app_info_body(&info))
    );
    let dlcs = vec![DlcInfo {
        app_id: 1001,
        name: Some("Saved".into()),
        content_ready: true,
        disabled: false,
    }];
    assert_eq!(
        game.ask(2, cmd::DLC_LIST, &[]).await,
        (status::OK, dlc_list_body(&dlcs))
    );
    assert_eq!(
        game.ask(3, cmd::SESSION_INFO, &[]).await.0,
        status::NO_SESSION
    );
    game.hang_up().await;

    // No saved DLC list.
    let (mut game, _) = Game::connect(fake, router, hello(70), None).await;
    assert_eq!(game.ask(1, cmd::DLC_LIST, &[]).await.0, status::NO_SESSION);
    assert_eq!(game.ask(2, cmd::APP_INFO, &[]).await.0, status::OK);
    game.hang_up().await;
}

#[tokio::test]
async fn ugc_subscriptions_and_state() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    for id in [42u64, 43] {
        let (code, _) = game.ask(1, cmd::UGC_SUBSCRIBE, &id.to_le_bytes()).await;
        assert_eq!(code, status::OK);
    }
    let (code, _) = game
        .ask(2, cmd::UGC_UNSUBSCRIBE, &43u64.to_le_bytes())
        .await;
    assert_eq!(code, status::OK);
    let (code, body) = game.ask(3, cmd::UGC_SUBSCRIBED, &[]).await;
    let mut expected = 1u32.to_le_bytes().to_vec();
    expected.extend_from_slice(&42u64.to_le_bytes());
    assert_eq!((code, body), (status::OK, expected));
    assert_eq!(
        game.ask(4, cmd::UGC_SUBSCRIBE, &[1]).await.0,
        status::MALFORMED
    );

    let (code, body) = game.ask(5, cmd::UGC_ITEM_STATE, &42u64.to_le_bytes()).await;
    let state = fake.ugc_state(480, 42).await.unwrap();
    assert_eq!((code, body), (status::OK, ugc_state_body(&state, None)));
    game.hang_up().await;
}

#[tokio::test]
async fn ugc_download_pushes_result() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router, hello(480), None).await;
    let mut body = 42u64.to_le_bytes().to_vec();
    body.push(1);
    assert_eq!(game.ask(1, cmd::UGC_DOWNLOAD, &body).await.0, status::OK);
    assert_eq!(
        game.next().await,
        (proto::C_SW_PUSH, ugc_downloaded_payload(480, 42, 1))
    );
    let mut body = 13u64.to_le_bytes().to_vec();
    body.push(0);
    assert_eq!(game.ask(2, cmd::UGC_DOWNLOAD, &body).await.0, status::OK);
    assert_eq!(
        game.next().await,
        (proto::C_SW_PUSH, ugc_downloaded_payload(480, 13, 2))
    );
    assert!(UGC_DOWNLOADS.lock().unwrap().is_empty());
    game.hang_up().await;
}

#[tokio::test]
async fn ugc_needs_a_session() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake.clone(), router, hello(480), None).await;
    *fake.steam_id.lock().unwrap() = None;
    for command in [
        cmd::UGC_QUERY,
        cmd::UGC_SUBSCRIBE,
        cmd::UGC_UNSUBSCRIBE,
        cmd::UGC_SUBSCRIBED,
        cmd::UGC_ITEM_STATE,
        cmd::UGC_DOWNLOAD,
    ] {
        let (code, _) = game
            .ask(u32::from(command), command, &42u64.to_le_bytes())
            .await;
        assert_eq!(code, status::NO_SESSION, "command {command}");
    }
    game.hang_up().await;
}

/// Same bytes the .so tests use.
#[test]
fn ugc_golden_bytes() {
    let mut item = Vec::new();
    put_ugc_item(&mut item, &sample_item());
    let mut expected = 42u64.to_le_bytes().to_vec();
    expected.extend_from_slice(&1i32.to_le_bytes());
    expected.extend_from_slice(&[0; 12]);
    expected.extend_from_slice(&[3, 0, 0, 0, b'M', b'a', b'p']);
    expected.extend_from_slice(&[0; 4]);
    expected.extend_from_slice(&[0; 8 + 12 + 2]);
    expected.extend_from_slice(&[4, 0, 0, 0, b'M', b'a', b'p', b's']);
    expected.extend_from_slice(&999u64.to_le_bytes());
    expected.extend_from_slice(&[0; 8 + 4 + 16 + 4 + 8]);
    expected.extend_from_slice(&0.5f32.to_bits().to_le_bytes());
    expected.extend_from_slice(&[0; 4]);
    assert_eq!(item, expected);
    let state = UgcInstall {
        installed: true,
        needs_update: false,
        size: 5,
        timestamp: 6,
        folder: "/f".into(),
    };
    assert_eq!(
        ugc_state_body(&state, Some((1, 2))),
        [
            5, 5, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0, 2, 0, 0, 0, b'/', b'f', //
            1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0,
        ]
    );
    assert_eq!(
        ugc_downloaded_payload(480, 42, 1),
        [70, 0xE0, 1, 0, 0, 42, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]
    );
}

#[tokio::test]
async fn avatar_round_trip_and_failures() {
    let fake = Fake::logged_in();
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router, hello(480), None).await;
    let ask = |id: u64, size: u8| {
        let mut body = id.to_le_bytes().to_vec();
        body.push(size);
        body
    };
    let (code, body) = game.ask(1, cmd::AVATAR, &ask(SID + 1, 0)).await;
    assert_eq!(code, status::OK);
    let mut expected = 2u32.to_le_bytes().to_vec();
    expected.extend_from_slice(&2u32.to_le_bytes());
    expected.extend_from_slice(&[7; 16]);
    assert_eq!(body, expected);
    assert_eq!(
        game.ask(2, cmd::AVATAR, &ask(SID + 1, 2)).await.1.len(),
        8 + 36
    );
    assert_eq!(
        game.ask(3, cmd::AVATAR, &ask(SID + 9, 1)).await.0,
        status::FAILED
    );
    assert_eq!(
        game.ask(4, cmd::AVATAR, &ask(SID + 1, 7)).await.0,
        status::MALFORMED
    );
    game.hang_up().await;
}

#[tokio::test]
async fn offline_avatars_fail_closed() {
    let fake = offline(&[(480, vec![0xCC])]);
    let router = Arc::new(Router::default());
    let (mut game, _) = Game::connect(fake, router, hello(480), None).await;
    let mut body = (SID + 1).to_le_bytes().to_vec();
    body.push(0);
    assert_eq!(game.ask(1, cmd::AVATAR, &body).await.0, status::NO_SESSION);
    game.hang_up().await;
}
