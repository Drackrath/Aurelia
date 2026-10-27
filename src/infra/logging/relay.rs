//! Output relay for commands the session daemon runs on behalf of a thin client.
//!
//! A forwarded command executes inside the long-lived `aurelia daemon`, whose own
//! stdout/stderr are discarded. Without this module two kinds of output never reached
//! the terminal that ran e.g. `aurelia play -v` (issue #34):
//!
//! * **tracing events** — the daemon's subscriber logs to its own (null) stderr at the
//!   daemon's verbosity, not the requester's `-v` count;
//! * **child process output** — a launched game/Proton/Wine inherited the daemon's
//!   (null) stdio.
//!
//! The daemon installs a [`Relay`] for each request via [`scoped`]. While it is
//! current, [`relay_layers`](super::cli) format tracing events at the *requester's*
//! verbosity into it, and child processes spawned with [`child_stdio`] + [`attach`]
//! have their stdout/stderr pumped into it. Outside a relay scope (a plain local CLI
//! run) everything behaves exactly as before: children inherit the real stdio.
//!
//! A request can additionally [`watch_daemon`] to receive the daemon's own output
//! (everything not raised by some request), prefixed `[daemon]`.

use std::future::Future;
use std::io::Read;
use std::sync::Arc;

/// Which client stream relayed bytes belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayStream {
    Stdout,
    Stderr,
}

/// Destination for relayed bytes. Must not keep the client connection alive on its
/// own: a child (or grandchild) may outlive the request.
pub type RelaySink = Arc<dyn Fn(RelayStream, &[u8]) + Send + Sync>;

/// Per-request relay context.
#[derive(Clone)]
pub struct Relay {
    /// The requester's `-v` count.
    pub verbosity: u8,
    sink: RelaySink,
}

impl Relay {
    pub fn new(verbosity: u8, sink: RelaySink) -> Self {
        Self { verbosity, sink }
    }

    fn send(&self, stream: RelayStream, bytes: &[u8]) {
        (self.sink)(stream, bytes);
    }
}

tokio::task_local! {
    static RELAY: Relay;
}

/// Run `fut` with `relay` as the current output relay.
pub async fn scoped<F: Future>(relay: Relay, fut: F) -> F::Output {
    RELAY.scope(relay, fut).await
}

/// The relay for the current task, if any.
pub fn current() -> Option<Relay> {
    RELAY.try_with(Relay::clone).ok()
}

/// The requester's verbosity when running inside a relay scope.
pub(super) fn current_verbosity() -> Option<u8> {
    RELAY.try_with(|r| r.verbosity).ok()
}

/// Send bytes to the current relay, if any. Returns whether a relay took them.
pub(super) fn send_current(stream: RelayStream, bytes: &[u8]) -> bool {
    RELAY.try_with(|r| r.send(stream, bytes)).is_ok()
}

/// `-v` count of a local (non-forwarded) CLI run, set by `init_cli_logging`.
static LOCAL_VERBOSITY: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub(super) fn set_local_verbosity(verbosity: u8) {
    LOCAL_VERBOSITY.store(verbosity, std::sync::atomic::Ordering::Relaxed);
}

/// Stdio for a game's (or its runner's) stdout/stderr. That output is diagnostic
/// noise unless asked for, so it is discarded below `-v`. With `-v` it is piped
/// inside a relay scope (then [`attach`] forwards it), otherwise inherited.
pub fn child_stdio() -> std::process::Stdio {
    match current() {
        Some(relay) if relay.verbosity > 0 => std::process::Stdio::piped(),
        Some(_) => std::process::Stdio::null(),
        None if LOCAL_VERBOSITY.load(std::sync::atomic::Ordering::Relaxed) > 0 => {
            std::process::Stdio::inherit()
        }
        None => std::process::Stdio::null(),
    }
}

/// Start forwarding a freshly spawned child's piped stdout/stderr to the current
/// relay. A no-op for streams that weren't piped or outside a relay scope.
///
/// The pumps are plain threads that drain until EOF even after the request ended (a
/// game launched by a wrapper can outlive it): stopping early would block or
/// SIGPIPE the game on a full pipe.
pub fn attach(mut child: std::process::Child) -> std::process::Child {
    let Some(relay) = current() else { return child };
    if let Some(out) = child.stdout.take() {
        pump(out, RelayStream::Stdout, relay.clone());
    }
    if let Some(err) = child.stderr.take() {
        pump(err, RelayStream::Stderr, relay);
    }
    child
}

fn pump<R: Read + Send + 'static>(mut src: R, stream: RelayStream, relay: Relay) {
    let spawned = std::thread::Builder::new()
        .name("aurelia-child-output".into())
        .spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match src.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => relay.send(stream, &buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("could not start child output relay: {e}");
    }
}

/// Requests subscribed to the daemon's own output (see [`watch_daemon`]).
static WATCHERS: std::sync::Mutex<Vec<(u64, Relay)>> = std::sync::Mutex::new(Vec::new());

/// Prefix marking relayed daemon output, so it can't be mistaken for the request's.
const DAEMON_PREFIX: &[u8] = b"[daemon] ";

/// Unsubscribes its request from the daemon's output when dropped.
pub struct DaemonWatch(u64);

impl Drop for DaemonWatch {
    fn drop(&mut self) {
        if let Ok(mut watchers) = WATCHERS.lock() {
            watchers.retain(|(id, _)| *id != self.0);
        }
    }
}

/// Also relay the daemon's *own* output — events raised outside any request (session
/// reconnects, liveness checks, …) and its direct stdout/stderr writes — to `relay`
/// until the returned guard is dropped. Tracing events are filtered by the relay's
/// verbosity like the request's own.
pub fn watch_daemon(relay: Relay) -> DaemonWatch {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut watchers) = WATCHERS.lock() {
        watchers.push((id, relay));
    }
    DaemonWatch(id)
}

/// Whether any watcher wants daemon tracing at `level`.
pub(super) fn has_daemon_watcher(level: u8, max: u8) -> bool {
    WATCHERS
        .lock()
        .is_ok_and(|w| w.iter().any(|(_, r)| r.verbosity.min(max) == level))
}

/// Send daemon output to every watcher, or only those at `level` when given.
fn broadcast(stream: RelayStream, bytes: &[u8], level: Option<(u8, u8)>) {
    let Ok(watchers) = WATCHERS.lock() else { return };
    let mut line = DAEMON_PREFIX.to_vec();
    line.extend_from_slice(bytes);
    for (_, relay) in watchers.iter() {
        if level.is_none_or(|(level, max)| relay.verbosity.min(max) == level) {
            relay.send(stream, &line);
        }
    }
}

/// Relay a direct (non-tracing) write the daemon made outside any request to all
/// watchers. Returns whether anyone was watching.
pub fn broadcast_daemon_output(stream: RelayStream, bytes: &[u8]) -> bool {
    let watching = WATCHERS.lock().is_ok_and(|w| !w.is_empty());
    if watching {
        broadcast(stream, bytes, None);
    }
    watching
}

/// `MakeWriter` for the daemon-watch tracing layers: each formatted event goes to the
/// watchers whose verbosity is `level` (capped at `max`).
#[derive(Clone, Copy)]
pub(super) struct DaemonWatchWriter {
    pub(super) level: u8,
    pub(super) max: u8,
}

impl std::io::Write for DaemonWatchWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        broadcast(RelayStream::Stderr, buf, Some((self.level, self.max)));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for DaemonWatchWriter {
    type Writer = DaemonWatchWriter;

    fn make_writer(&'a self) -> Self::Writer {
        *self
    }
}

/// `MakeWriter` for the daemon's relay tracing layers: each formatted event goes to
/// the current request's stderr.
#[derive(Clone, Copy, Default)]
pub(super) struct RelayWriter;

impl std::io::Write for RelayWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        send_current(RelayStream::Stderr, buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RelayWriter {
    type Writer = RelayWriter;

    fn make_writer(&'a self) -> Self::Writer {
        RelayWriter
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
