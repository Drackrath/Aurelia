use super::*;
use std::sync::Mutex as StdMutex;

/// Records sent messages.
#[derive(Clone, Default)]
struct FakeLink {
    state: Option<PlayingSession>,
    fail: bool,
    sent: Arc<StdMutex<Vec<CMsgClientGamesPlayed>>>,
}

impl FakeLink {
    fn with_state(blocked: bool, playing_app: u32) -> Self {
        Self {
            state: Some(PlayingSession {
                blocked,
                playing_app,
            }),
            ..Self::default()
        }
    }

    fn sent(&self) -> Vec<CMsgClientGamesPlayed> {
        self.sent.lock().unwrap().clone()
    }

    fn app_ids(msg: &CMsgClientGamesPlayed) -> Vec<u64> {
        msg.games_played.iter().map(|g| g.game_id()).collect()
    }
}

impl PlayingLink for FakeLink {
    async fn playing_state(&self) -> Option<PlayingSession> {
        self.state
    }

    async fn send_games_played(&self, msg: CMsgClientGamesPlayed) -> Result<()> {
        if self.fail {
            bail!("connection closed");
        }
        self.sent.lock().unwrap().push(msg);
        Ok(())
    }
}

fn launch(app_id: u32) -> Launch {
    Launch { app_id, pid: None }
}

fn leaked() -> &'static Mutex<PlayingRegistry> {
    Box::leak(Box::new(Mutex::new(PlayingRegistry::new())))
}

#[test]
fn message_carries_app_and_pid() {
    let msg = games_played_message(&[Launch {
        app_id: 440,
        pid: Some(1234),
    }]);
    assert_eq!(msg.client_os_type(), CLIENT_OS_TYPE);
    assert_eq!(msg.games_played.len(), 1);
    assert_eq!(msg.games_played[0].game_id(), 440);
    assert_eq!(msg.games_played[0].process_id(), 1234);
}

#[test]
fn message_lists_each_app_once() {
    let msg = games_played_message(&[launch(440), launch(570), launch(440)]);
    assert_eq!(FakeLink::app_ids(&msg), vec![440, 570]);
}

#[test]
fn empty_message_clears_playing() {
    assert!(games_played_message(&[]).games_played.is_empty());
}

#[tokio::test]
async fn start_then_stop_reports_and_clears() {
    let link = FakeLink::with_state(false, 0);
    let mut reg = PlayingRegistry::new();
    let id = reg.start(&link, link.state, launch(440)).await.unwrap();
    assert!(reg.stop(Some(&link), id).await.unwrap());
    let sent = link.sent();
    assert_eq!(sent.len(), 2);
    assert_eq!(FakeLink::app_ids(&sent[0]), vec![440]);
    assert!(sent[1].games_played.is_empty());
    assert!(reg.launches().is_empty());
}

#[tokio::test]
async fn stopping_one_keeps_the_other() {
    let link = FakeLink::with_state(false, 0);
    let mut reg = PlayingRegistry::new();
    let a = reg.start(&link, link.state, launch(440)).await.unwrap();
    reg.start(&link, link.state, launch(570)).await.unwrap();
    reg.stop(Some(&link), a).await.unwrap();
    let sent = link.sent();
    assert_eq!(FakeLink::app_ids(&sent[1]), vec![440, 570]);
    assert_eq!(FakeLink::app_ids(&sent[2]), vec![570]);
}

#[tokio::test]
async fn blocked_session_refuses_with_clear_message() {
    let link = FakeLink::with_state(true, 440);
    let mut reg = PlayingRegistry::new();
    let err = reg
        .start(&link, link.state, launch(570))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("playing app 440"), "{err}");
    assert!(err.contains("one playing session per account"), "{err}");
    assert!(err.contains("app 570 was not started"), "{err}");
    assert!(link.sent().is_empty());
    assert!(reg.launches().is_empty());
}

#[test]
fn blocked_without_app_refuses() {
    let err = check_conflict(
        Some(PlayingSession {
            blocked: true,
            playing_app: 0,
        }),
        570,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("holds the playing session"), "{err}");
}

#[test]
fn own_session_playing_is_allowed() {
    let own = Some(PlayingSession {
        blocked: false,
        playing_app: 440,
    });
    assert!(check_conflict(own, 570).is_ok());
}

#[test]
fn unknown_state_is_allowed() {
    assert!(check_conflict(None, 570).is_ok());
}

#[tokio::test]
async fn failed_send_rolls_back() {
    let link = FakeLink {
        fail: true,
        ..FakeLink::default()
    };
    let mut reg = PlayingRegistry::new();
    assert!(reg.start(&link, None, launch(440)).await.is_err());
    assert!(reg.launches().is_empty());
}

#[tokio::test]
async fn unknown_id_sends_nothing() {
    let link = FakeLink::default();
    let mut reg = PlayingRegistry::new();
    assert!(!reg.stop(Some(&link), 7).await.unwrap());
    assert!(link.sent().is_empty());
}

#[tokio::test]
async fn stop_without_session_just_forgets() {
    let link = FakeLink::default();
    let mut reg = PlayingRegistry::new();
    let id = reg.start(&link, None, launch(440)).await.unwrap();
    assert!(reg.stop(None::<&FakeLink>, id).await.unwrap());
    assert_eq!(link.sent().len(), 1);
    assert!(reg.launches().is_empty());
}

#[tokio::test]
async fn resend_reports_active_games_only() {
    let old = FakeLink::default();
    let fresh = FakeLink::default();
    let mut reg = PlayingRegistry::new();
    reg.resend(&fresh).await.unwrap();
    assert!(fresh.sent().is_empty());
    reg.start(&old, None, launch(440)).await.unwrap();
    reg.resend(&fresh).await.unwrap();
    assert_eq!(FakeLink::app_ids(&fresh.sent()[0]), vec![440]);
}

#[tokio::test]
async fn start_launch_needs_a_session() {
    let reg = leaked();
    let err = start_launch(reg, launch(440), || async { None::<FakeLink> })
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("not logged in"), "{err}");
    assert!(reg.lock().await.launches().is_empty());
}

#[tokio::test]
async fn start_launch_refuses_dead_pid() {
    let link = FakeLink::default();
    let reg = leaked();
    let fake = link.clone();
    let dead = Launch {
        app_id: 440,
        pid: Some(u32::MAX - 1),
    };
    let res = start_launch(reg, dead, move || {
        let l = fake.clone();
        async move { Some(l) }
    })
    .await;
    assert!(res.is_err());
    assert!(link.sent().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn killed_game_clears_games_played() {
    let mut child = tokio::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");
    let pid = child.id().expect("child pid");
    let link = FakeLink::default();
    let reg = leaked();
    let fake = link.clone();
    let game = Launch {
        app_id: 440,
        pid: Some(pid),
    };
    start_launch_polling(
        reg,
        game,
        move || {
            let l = fake.clone();
            async move { Some(l) }
        },
        Duration::from_millis(20),
    )
    .await
    .unwrap();
    assert_eq!(link.sent().len(), 1);

    child.kill().await.expect("kill sleep");
    let deadline = Instant::now() + Duration::from_secs(5);
    while link.sent().len() < 2 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let sent = link.sent();
    assert_eq!(sent.len(), 2, "exit was not reported");
    assert!(sent[1].games_played.is_empty());
    assert!(reg.lock().await.launches().is_empty());
}

#[cfg(unix)]
#[test]
fn zombie_counts_as_exited() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");
    let pid = child.id();
    assert!(process_identity(pid).is_some());
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    // Unreaped child stays a zombie.
    std::thread::sleep(Duration::from_millis(200));
    assert!(process_identity(pid).is_none());
    child.wait().unwrap();
}
