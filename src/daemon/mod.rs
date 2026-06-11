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

use std::sync::OnceLock;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use aurelia::steam_client::SteamClient;

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

struct DaemonState {
    slot: RwLock<Slot>,
}

struct Slot {
    /// The canonical authenticated client holding the live shared connection.
    client: Option<SteamClient>,
    /// `session.json` mtime the current `client`/failure reflects — so we re-attempt
    /// a restore exactly when the token file changes (e.g. after `login`), rather
    /// than re-logging-on per request (which would re-create the storm).
    session_mtime: Option<SystemTime>,
    /// Whether the last restore attempt for `session_mtime` failed (so we don't
    /// retry a known-bad token until the file changes).
    last_attempt_failed: bool,
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
            last_attempt_failed: false,
        }),
    })
}

async fn session_mtime() -> Option<SystemTime> {
    let path = aurelia::config::config_dir().ok()?.join("session.json");
    tokio::fs::metadata(&path).await.ok()?.modified().ok()
}

impl DaemonState {
    /// Ensure the shared session reflects the current `session.json`. Restores once
    /// when the token file appears or changes; otherwise a no-op (no per-request
    /// logon). Failures are remembered so a bad token isn't retried in a loop.
    async fn ensure_session(&self) {
        let mtime = session_mtime().await;
        {
            let s = self.slot.read().await;
            if s.session_mtime == mtime && (s.client.is_some() || s.last_attempt_failed) {
                return;
            }
        }
        let mut s = self.slot.write().await;
        // Re-check under the write lock (another task may have just restored).
        if s.session_mtime == mtime && (s.client.is_some() || s.last_attempt_failed) {
            return;
        }

        match SteamClient::new() {
            Ok(mut client) => match client.restore_session().await {
                Ok(_) if client.is_authenticated() => {
                    tracing::info!("daemon: shared Steam session established");
                    s.client = Some(client);
                    s.last_attempt_failed = false;
                }
                Ok(_) => {
                    tracing::warn!("daemon: session restore did not authenticate");
                    s.client = None;
                    s.last_attempt_failed = true;
                }
                Err(e) => {
                    tracing::warn!("daemon: could not restore shared session: {e:#}");
                    s.client = None;
                    s.last_attempt_failed = true;
                }
            },
            Err(e) => {
                tracing::warn!("daemon: could not build Steam client: {e:#}");
                s.client = None;
                s.last_attempt_failed = true;
            }
        }
        s.session_mtime = mtime;
    }

    /// Drop the shared session and clear the failure latch so the next
    /// [`ensure_session`] re-attempts from scratch (used after login/logout).
    async fn invalidate(&self) {
        let mut s = self.slot.write().await;
        s.client = None;
        s.last_attempt_failed = false;
        s.session_mtime = None;
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
            account: aurelia::config::load_session()
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
    if touches_session {
        if let Some(state) = DAEMON.get() {
            state.invalidate().await;
            state.ensure_session().await;
        }
    }
}
