use super::*;
use anyhow::bail;
use tokio::io::DuplexStream;

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
    writes: bool,
    stored: Mutex<Vec<(u32, u32, bool, StatChanges)>>,
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
