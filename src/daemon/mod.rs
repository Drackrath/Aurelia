//! Persistent session daemon.
//!
//! The per-invocation CLI re-authenticates with Steam on every command, and Steam
//! throttles repeated logons (the `invalid credentials` / `RateLimitExceeded`
//! churn). This module lets **one** long-running `aurelia daemon` process hold a
//! single authenticated Steam session and execute every other `aurelia` command on
//! its behalf, so the whole machine logs on once per daemon lifetime.
//!
//! - [`server`] accepts forwarded commands and runs them against the shared session.
//! - [`client`] is what a normal `aurelia <cmd>` uses to forward itself to a running
//!   daemon (auto-spawning one if needed), transparently relaying stdio + exit code.
//! - The shared session lives in [`DaemonState`]; [`shared_restored_client`] hands
//!   each request a cheap clone-backed [`SteamClient`] over the one live connection.

pub mod client;
mod proto;
mod server;
mod transport;

use std::sync::Arc;
use std::sync::OnceLock;
// The roster cache is shared with the (synchronous) friends watcher
use std::sync::RwLock as StdRwLock;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use aurelia::steam_client::{Friend, Roster, SteamClient};

pub use server::run_server;

/// The request header the client sends first: the command's argv (including the
/// program name at index 0, so the daemon can `Cli::try_parse_from` it directly).
#[derive(Debug, Serialize, Deserialize)]
struct Header {
    argv: Vec<String>,
}

/// Process-global daemon session. Present (`Some`) only inside the daemon process;
/// a normal CLI invocation never initializes it, which is how [`in_daemon`]
/// distinguishes the two.
static DAEMON: OnceLock<DaemonState> = OnceLock::new();

/// After a failed session restore, wait at least this long before the next request
/// is allowed to retry. Long enough that a genuinely bad/throttled token can't
/// re-create the per-request logon storm the daemon exists to prevent, short enough
/// that a *transient* failure (CM blip, timeout during a heavy download) self-heals
/// on a later command instead of wedging the daemon until `login --reconnect`.
const RESTORE_RETRY_BACKOFF: Duration = Duration::from_secs(30);

/// How often the background liveness loop probes the shared connection. A dropped
/// socket is otherwise invisible (steam-vent leaves the `Connection` API-usable), so
/// without this the session only heals when a command happens to fail on it.
const LIVENESS_PROBE_INTERVAL: Duration = Duration::from_secs(60);

/// After a failed probe, wait this long and probe once more before reconnecting — a
/// single failure can be a transient hiccup, and a needless reconnect drops a working
/// session and burns a logon.
const LIVENESS_RECHECK_DELAY: Duration = Duration::from_secs(2);

struct DaemonState {
    slot: RwLock<Slot>,
    /// The shared friends cache. Populated asynchronously by the single watcher task
    /// (see [`DaemonState::spawn_watcher`]) from Steam friend/persona broadcasts, and
    /// read by [`shared_roster`]. An `Arc` so the watcher task can own a handle to the
    /// same map that survives session teardown (the Arc outlives any one watcher).
    roster: Arc<StdRwLock<Roster>>,
    /// Handle to the single in-flight watcher task. Kept so we never spawn a duplicate
    /// watcher while one is still running, and so [`invalidate`](Self::invalidate) can
    /// abort it when the session is torn down. A std `Mutex` (not tokio) because it is
    /// only locked for the instant it takes to inspect/replace the handle, never across
    /// an `.await`.
    watcher: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

struct Slot {
    /// The canonical authenticated client holding the live shared connection.
    client: Option<SteamClient>,
    /// `session.json` mtime the current `client`/failure reflects — so we re-attempt
    /// a restore exactly when the token file changes (e.g. after `login`), rather
    /// than re-logging-on per request (which would re-create the storm).
    session_mtime: Option<SystemTime>,
    /// When the last restore attempt for `session_mtime` failed, if it did. Gates
    /// retries to one per [`RESTORE_RETRY_BACKOFF`]: a transient failure is retried
    /// (and self-heals) once the window elapses, but a persistently failing token is
    /// not retried fast enough to re-create the logon storm. `None` means the last
    /// attempt succeeded or none has been made.
    last_failure: Option<Instant>,
}

impl Slot {
    /// Record that the restore for the current `mtime` failed: drop any client and
    /// arm the retry backoff. The matching `tracing::warn!` is emitted by the caller
    /// (each failure path has its own message); this only mutates the slot.
    fn record_failure(&mut self) {
        self.client = None;
        self.last_failure = Some(Instant::now());
    }

    /// Whether the slot already reflects `mtime` and needs no restore attempt now:
    /// either a live client exists, or a recent failure is still within its backoff.
    fn is_current(&self, mtime: Option<SystemTime>) -> bool {
        if self.session_mtime != mtime {
            return false; // token file changed — must re-restore.
        }
        if self.client.is_some() {
            return true; // healthy session already established.
        }
        // No client: retry, unless a failure is still inside the backoff window.
        self.last_failure
            .is_some_and(|at| at.elapsed() < RESTORE_RETRY_BACKOFF)
    }
}

/// Whether this process is the daemon (vs. a thin client / standalone run).
pub fn in_daemon() -> bool {
    DAEMON.get().is_some()
}

fn init_state() -> &'static DaemonState {
    DAEMON.get_or_init(|| DaemonState {
        slot: RwLock::new(Slot {
            client: None,
            session_mtime: None,
            last_failure: None,
        }),
        roster: Arc::new(StdRwLock::new(Roster::new())),
        watcher: std::sync::Mutex::new(None),
    })
}

async fn session_mtime() -> Option<SystemTime> {
    let path = aurelia::core::config::config_dir().ok()?.join("session.json");
    tokio::fs::metadata(&path).await.ok()?.modified().ok()
}

impl DaemonState {
    /// Ensure the shared session reflects the current `session.json`. Restores when
    /// the token file appears or changes; otherwise a no-op (no per-request logon)
    /// while a live session exists. A failed restore is retried on a later request
    /// once [`RESTORE_RETRY_BACKOFF`] has elapsed, so a transient drop self-heals
    /// without re-creating the logon storm.
    async fn ensure_session(&self) {
        let mtime = session_mtime().await;
        {
            let s = self.slot.read().await;
            if s.is_current(mtime) {
                return;
            }
        }
        let mut s = self.slot.write().await;
        // Re-check under the write lock (another task may have just restored).
        if s.is_current(mtime) {
            return;
        }

        match SteamClient::new() {
            Ok(mut client) => match client.restore_session().await {
                Ok(_) if client.is_authenticated() => {
                    tracing::info!("daemon: shared Steam session established");
                    s.client = Some(client);
                    s.last_failure = None;
                    // Start (or re-confirm) the background friends watcher on the freshly
                    // established connection so the roster cache fills without a request
                    // having to drive it. `spawn_watcher` is idempotent — if one is still
                    // running (e.g. this restore ran without a prior invalidate) it's a
                    // no-op. `s.client` was just set, so the unwrap cannot fail.
                    self.spawn_watcher(s.client.as_ref().unwrap());
                }
                Ok(_) => {
                    tracing::warn!("daemon: session restore did not authenticate");
                    s.record_failure();
                }
                Err(e) => {
                    tracing::warn!("daemon: could not restore shared session: {e:#}");
                    s.record_failure();
                }
            },
            Err(e) => {
                tracing::warn!("daemon: could not build Steam client: {e:#}");
                s.record_failure();
            }
        }
        s.session_mtime = mtime;
    }

    /// Spawn the background friends watcher on `client`, unless one is already running.
    ///
    /// The watcher runs forever against the client's connection, populating the shared
    /// [`roster`](Self::roster) from Steam friend/persona broadcasts, and returns only
    /// when that connection's streams end (i.e. the session died). We keep exactly one
    /// running at a time: a stale-but-not-yet-cleaned handle (the task already finished,
    /// e.g. its connection dropped) is treated as "no watcher" so we replace it. Locked
    /// for just the inspect/replace; the spawn and `.await` happen on the task itself.
    fn spawn_watcher(&self, client: &SteamClient) {
        let mut guard = self.watcher.lock().unwrap();
        if guard.as_ref().is_some_and(|h| !h.is_finished()) {
            return; // a watcher is already live on this (or a prior) connection.
        }
        // Both the client (cheap Arc-backed clone over the shared connection) and the
        // roster Arc are moved into the task so it can outlive this call.
        let client = client.clone();
        let roster = Arc::clone(&self.roster);
        let handle = tokio::spawn(async move {
            if let Err(e) = client.run_friends_watcher(roster).await {
                tracing::warn!("daemon: friends watcher exited: {e:#}");
            }
        });
        tracing::info!("daemon: friends watcher started");
        *guard = Some(handle);
    }

    /// Drop the shared session and clear the failure backoff so the next
    /// [`ensure_session`] re-attempts immediately (used after login/logout). Also tears
    /// down the friends watcher and empties the roster: the watcher is bound to the dead
    /// connection, so a reconnect must start a fresh one over the new connection rather
    /// than leave a defunct task and stale friends data behind.
    async fn invalidate(&self) {
        let mut s = self.slot.write().await;
        s.client = None;
        s.last_failure = None;
        s.session_mtime = None;
        // Abort the watcher bound to the now-dead connection and drop its handle so the
        // next `ensure_session` spawns a fresh one. (A held std lock, but no `.await`
        // occurs while it is held.)
        if let Some(handle) = self.watcher.lock().unwrap().take() {
            handle.abort();
        }
        // Clear stale friends data so a read between teardown and the next watcher's
        // first broadcast doesn't return friends from the previous session.
        self.roster.write().unwrap().clear();
    }

    /// Periodically verify the shared connection is still alive and re-establish it if
    /// not. steam-vent leaves a `Connection` usable in its API after the socket dies,
    /// so a dropped session is otherwise invisible until a command fails on it. On a
    /// confirmed-dead connection this re-establishes the session in the background so
    /// the next command doesn't have to eat the failure first. Never returns.
    async fn liveness_loop(&self) {
        loop {
            tokio::time::sleep(LIVENESS_PROBE_INTERVAL).await;

            // Healthy, or nothing to probe — wait for the next tick.
            if self.probe_once().await {
                continue;
            }
            // One failure can be a transient hiccup; confirm before paying for a
            // reconnect (which drops a possibly-fine session and burns a logon).
            tokio::time::sleep(LIVENESS_RECHECK_DELAY).await;
            if self.probe_once().await {
                continue;
            }

            tracing::warn!("daemon: shared session failed liveness probe; reconnecting");
            self.invalidate().await;
            self.ensure_session().await;
        }
    }

    /// Probe the current shared connection once. Returns `true` when it is healthy
    /// **or** there is no session to probe (nothing to do); `false` only when a
    /// live-looking session failed the probe and should be re-established.
    async fn probe_once(&self) -> bool {
        // Snapshot the (cheap, Arc-backed) client so the probe doesn't hold the lock.
        let client = self.slot.read().await.client.clone();
        match client {
            Some(client) => client.probe_alive().await.is_ok(),
            None => true,
        }
    }
}

/// Daemon-side replacement for `restored_client()`: returns a client backed by the
/// shared connection (no logon) when a session exists, or an unauthenticated client
/// otherwise (so `authed_client()` then reports "not logged in" as usual).
pub async fn shared_restored_client() -> SteamClient {
    let state = init_state();
    state.ensure_session().await;
    let slot = state.slot.read().await;
    match slot.client.as_ref().and_then(|c| c.connection().cloned()) {
        Some(connection) => SteamClient::from_shared(connection),
        None => SteamClient::new().expect("SteamClient::new is infallible"),
    }
}

/// The daemon's shared friends list: every actual friend (`relationship == 3`) in the
/// roster cache, sorted by persona name (those without a name last) then steam id.
///
/// Ensures the shared session — and therefore the background watcher — is running, but
/// the roster is populated **asynchronously**: the watcher fills it as Steam pushes the
/// friends list and persona states over the connection. A call made immediately after
/// the daemon (re)establishes a session may therefore return an empty or partial list
/// until those broadcasts arrive; a slightly later call returns the full roster.
pub async fn shared_roster() -> Vec<Friend> {
    let state = init_state();
    state.ensure_session().await;
    // Brief read lock: clone out the friends we care about, then release before sorting.
    let mut friends: Vec<Friend> = {
        let roster = state.roster.read().unwrap();
        roster
            .values()
            .filter(|f| f.relationship == 3)
            .cloned()
            .collect()
    };
    // Stable, predictable ordering for callers/CLI output: by name (unnamed entries —
    // personas not yet received — sort last), tie-broken by the always-present steam id.
    friends.sort_by(|a, b| match (&a.persona_name, &b.persona_name) {
        (Some(x), Some(y)) => x.cmp(y).then(a.steam_id.cmp(&b.steam_id)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.steam_id.cmp(&b.steam_id),
    });
    friends
}

/// Snapshot of the daemon's shared session, for `login --health` / `--reconnect`.
#[derive(Debug, Default)]
pub struct SessionStatus {
    pub authenticated: bool,
    pub account: Option<String>,
    pub steam_id: Option<u64>,
}

async fn status_from_slot(slot: &Slot) -> SessionStatus {
    match &slot.client {
        Some(client) => SessionStatus {
            authenticated: true,
            account: aurelia::core::config::load_session()
                .await
                .ok()
                .and_then(|s| s.account_name),
            steam_id: client.steam_id(),
        },
        None => SessionStatus::default(),
    }
}

/// Report the shared session's health, (re)establishing it from the stored token if
/// that hasn't been tried yet — but never re-logging-on a session that is already up
/// or a token already known to be bad.
pub async fn session_status() -> SessionStatus {
    let state = init_state();
    state.ensure_session().await;
    let slot = state.slot.read().await;
    status_from_slot(&slot).await
}

/// Force the shared session to be torn down and re-established from the stored token
/// (e.g. after the live connection dropped). Returns the resulting status.
pub async fn force_reconnect() -> SessionStatus {
    let state = init_state();
    state.invalidate().await;
    state.ensure_session().await;
    let slot = state.slot.read().await;
    status_from_slot(&slot).await
}

/// After a `login`/`logout` command, refresh the shared session immediately so the
/// next request sees the new (or cleared) token without waiting on a file-mtime tick.
/// `login --health` (read-only) and `login --reconnect` (handles its own refresh) are
/// excluded so a status check never triggers a re-logon.
async fn maybe_refresh_after(argv: &[String]) {
    if argv.iter().any(|a| a == "--health" || a == "--reconnect") {
        return;
    }
    let touches_session = argv.iter().any(|a| a == "login" || a == "logout");
    if let Some(state) = DAEMON.get().filter(|_| touches_session) {
        state.invalidate().await;
        state.ensure_session().await;
    }
}

#[cfg(test)]
#[path = "daemon_tests.rs"]
mod tests;
