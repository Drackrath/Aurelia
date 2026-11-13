//! Report running games to Steam.
//!
//! Stand-in launches report `CMsgClientGamesPlayed`.
use super::*;
use std::collections::BTreeMap;
use std::future::Future;
use steam_vent_proto::steammessages_clientserver::CMsgClientGamesPlayed;
use steam_vent_proto::steammessages_clientserver::cmsg_client_games_played::GamePlayed;
use tokio::sync::Mutex;

/// Same OS type as logon.
const CLIENT_OS_TYPE: u32 = 203;

/// Poll interval for game exit.
const EXIT_POLL: Duration = Duration::from_secs(2);

/// One running game we reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Launch {
    /// Steam app id.
    pub app_id: u32,
    /// Game process; None means caller-managed.
    pub pid: Option<u32>,
}

/// Session that reports games played.
pub trait PlayingLink: Send + Sync {
    /// Steam's account-wide playing state.
    fn playing_state(&self) -> impl Future<Output = Option<PlayingSession>> + Send;
    /// Send one games-played message.
    fn send_games_played(
        &self,
        msg: CMsgClientGamesPlayed,
    ) -> impl Future<Output = Result<()>> + Send;
}

impl PlayingLink for SteamClient {
    async fn playing_state(&self) -> Option<PlayingSession> {
        self.fetch_playing_session().await
    }

    async fn send_games_played(&self, msg: CMsgClientGamesPlayed) -> Result<()> {
        self.require_connection()?
            .send(msg)
            .await
            .map_err(|e| anyhow!("failed to report games played to Steam: {e}"))
    }
}

/// Refuse when another session plays.
pub fn check_conflict(state: Option<PlayingSession>, app_id: u32) -> Result<()> {
    match state {
        Some(s) if s.blocked && s.playing_app != 0 => bail!(
            "another Steam client on this account is playing app {} right now (likely \
             Valve's Steam), and Steam allows one playing session per account; app {app_id} \
             was not started. Quit that game there and launch again",
            s.playing_app
        ),
        Some(s) if s.blocked => bail!(
            "another Steam client on this account holds the playing session, and Steam \
             allows one per account; app {app_id} was not started. Quit the game running \
             there and launch again"
        ),
        Some(_) => Ok(()),
        None => {
            tracing::warn!(
                app_id,
                "Steam did not report the playing session; reporting the game anyway"
            );
            Ok(())
        }
    }
}

/// Build a games-played message.
pub fn games_played_message(launches: &[Launch]) -> CMsgClientGamesPlayed {
    let mut msg = CMsgClientGamesPlayed::new();
    msg.set_client_os_type(CLIENT_OS_TYPE);
    let mut seen = std::collections::HashSet::new();
    for launch in launches {
        // One entry per app.
        if !seen.insert(launch.app_id) {
            continue;
        }
        let mut game = GamePlayed::new();
        game.set_game_id(u64::from(launch.app_id));
        if let Some(pid) = launch.pid {
            game.set_process_id(pid);
        }
        msg.games_played.push(game);
    }
    msg
}

/// Launches this daemon reported.
#[derive(Debug, Default)]
pub struct PlayingRegistry {
    next_id: u64,
    active: BTreeMap<u64, Launch>,
}

impl PlayingRegistry {
    /// Empty registry.
    pub const fn new() -> Self {
        Self {
            next_id: 0,
            active: BTreeMap::new(),
        }
    }

    /// Active launches, oldest first.
    pub fn launches(&self) -> Vec<Launch> {
        self.active.values().copied().collect()
    }

    /// Check conflict, add, report.
    pub async fn start<L: PlayingLink>(
        &mut self,
        link: &L,
        state: Option<PlayingSession>,
        launch: Launch,
    ) -> Result<u64> {
        check_conflict(state, launch.app_id)?;
        self.next_id += 1;
        let id = self.next_id;
        self.active.insert(id, launch);
        if let Err(e) = link
            .send_games_played(games_played_message(&self.launches()))
            .await
        {
            self.active.remove(&id);
            return Err(e);
        }
        Ok(id)
    }

    /// Remove; report remaining games.
    pub async fn stop<L: PlayingLink>(&mut self, link: Option<&L>, id: u64) -> Result<bool> {
        if self.active.remove(&id).is_none() {
            return Ok(false);
        }
        // No session: Steam already cleared.
        if let Some(link) = link {
            link.send_games_played(games_played_message(&self.launches()))
                .await?;
        }
        Ok(true)
    }

    /// Re-report after a reconnect.
    pub async fn resend<L: PlayingLink>(&self, link: &L) -> Result<()> {
        if self.active.is_empty() {
            return Ok(());
        }
        link.send_games_played(games_played_message(&self.launches()))
            .await
    }
}

/// The daemon's launch registry.
pub static GAMES: Mutex<PlayingRegistry> = Mutex::const_new(PlayingRegistry::new());

/// Process identity: start time, unless gone.
pub fn process_identity(pid: u32) -> Option<u64> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.process(pid)
        .filter(|p| !matches!(p.status(), ProcessStatus::Zombie | ProcessStatus::Dead))
        .map(|p| p.start_time())
}

/// Wait until `pid` exits.
pub async fn wait_for_exit(pid: u32, born: u64, poll: Duration) {
    while process_identity(pid) == Some(born) {
        tokio::time::sleep(poll).await;
    }
}

/// Report a launch; clear on exit.
///
/// `link` yields the current session.
pub async fn start_launch<L, P, Fut>(
    registry: &'static Mutex<PlayingRegistry>,
    launch: Launch,
    link: P,
) -> Result<u64>
where
    L: PlayingLink + 'static,
    P: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Option<L>> + Send,
{
    start_launch_polling(registry, launch, link, EXIT_POLL).await
}

/// [`start_launch`] with a custom poll.
pub async fn start_launch_polling<L, P, Fut>(
    registry: &'static Mutex<PlayingRegistry>,
    launch: Launch,
    link: P,
    poll: Duration,
) -> Result<u64>
where
    L: PlayingLink + 'static,
    P: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Option<L>> + Send,
{
    let born = match launch.pid {
        Some(pid) => Some(
            process_identity(pid).ok_or_else(|| anyhow!("game process {pid} is not running"))?,
        ),
        None => None,
    };
    let session = link()
        .await
        .ok_or_else(|| anyhow!("not logged in — run `aurelia login` first"))?;
    let state = session.playing_state().await;
    let id = registry.lock().await.start(&session, state, launch).await?;
    if let (Some(pid), Some(born)) = (launch.pid, born) {
        tokio::spawn(async move {
            wait_for_exit(pid, born, poll).await;
            if let Err(e) = stop_launch(registry, id, &link).await {
                tracing::warn!(pid, "could not clear games played: {e:#}");
            }
        });
    }
    Ok(id)
}

/// Clear one launch from Steam.
pub async fn stop_launch<L, P, Fut>(
    registry: &'static Mutex<PlayingRegistry>,
    id: u64,
    link: &P,
) -> Result<bool>
where
    L: PlayingLink,
    P: Fn() -> Fut,
    Fut: Future<Output = Option<L>>,
{
    let session = link().await;
    registry.lock().await.stop(session.as_ref(), id).await
}

#[cfg(test)]
#[path = "playing_tests.rs"]
mod tests;
