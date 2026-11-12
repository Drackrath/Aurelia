//! Daemon server: accept forwarded commands and run them against the shared session.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use super::transport::{self, Listener};
use super::{Header, proto};
use crate::output::{OutChunk, OutputCtx, Stream};

/// Number of forwarded commands currently running, so the upgrade watcher only
/// restarts the daemon while it is idle (never mid-request).
static INFLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Unix seconds of last request activity.
static LAST_ACTIVITY: AtomicU64 = AtomicU64::new(0);

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn touch_activity() {
    LAST_ACTIVITY.store(now_secs(), Ordering::SeqCst);
}

/// Increments [`INFLIGHT`] for its lifetime; decrements on drop (covers `?` early
/// returns in [`handle`]).
struct InflightGuard;

impl InflightGuard {
    fn new() -> Self {
        INFLIGHT.fetch_add(1, Ordering::SeqCst);
        touch_activity();
        Self
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        INFLIGHT.fetch_sub(1, Ordering::SeqCst);
        touch_activity();
    }
}

/// `(mtime, len)` of the running executable, used to detect an on-disk upgrade.
///
/// After a rebuild replaces the binary, Linux reports `current_exe()` with a
/// ` (deleted)` suffix; stripping it makes the stat hit the *new* binary at the
/// original path, which is exactly the change this watcher exists to detect.
fn exe_signature() -> Option<(SystemTime, u64)> {
    let exe = std::env::current_exe().ok()?;
    let exe = match exe.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(stripped) => std::path::PathBuf::from(stripped),
        None => exe,
    };
    let meta = std::fs::metadata(&exe).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Watch the daemon's own binary; when it changes on disk (an `aurelia` upgrade)
/// exit once idle so the next forwarded command auto-spawns a daemon running the
/// new code. Without this a long-lived daemon keeps serving stale code — e.g. it
/// would reject a newly added subcommand with "unrecognized subcommand".
async fn watch_for_upgrade(startup: Option<(SystemTime, u64)>) {
    let Some(startup) = startup else { return };
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let changed = exe_signature().is_some_and(|current| current != startup);
        if changed && INFLIGHT.load(Ordering::SeqCst) == 0 {
            tracing::info!(
                "daemon: binary changed on disk; exiting so the next command starts a \
                 fresh daemon"
            );
            shut_down();
        }
    }
}

/// Clear marker and socket, then exit.
fn shut_down() -> ! {
    super::clear_daemon_info();
    transport::remove_endpoint();
    std::process::exit(0);
}

/// `AURELIA_DAEMON_IDLE_TIMEOUT` minutes, 0 off.
fn idle_timeout() -> Option<Duration> {
    std::env::var("AURELIA_DAEMON_IDLE_TIMEOUT")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&minutes| minutes > 0)
        .map(|minutes| Duration::from_secs(minutes.saturating_mul(60)))
}

/// Whether an idle daemon should exit.
fn idle_expired(last: u64, now: u64, timeout: Duration, busy: bool) -> bool {
    !busy && now.saturating_sub(last) >= timeout.as_secs()
}

/// Requests, installs or games still active.
fn busy() -> bool {
    INFLIGHT.load(Ordering::SeqCst) > 0
        || crate::commands::common::active_installs()
            .lock()
            .map_or(true, |map| !map.is_empty())
        || !aurelia::compat::running::list().is_empty()
}

/// Exit after the configured idle time.
async fn watch_for_idle(timeout: Duration) {
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let busy = busy();
        if busy {
            touch_activity();
        }
        if idle_expired(
            LAST_ACTIVITY.load(Ordering::SeqCst),
            now_secs(),
            timeout,
            busy,
        ) {
            tracing::info!("daemon: idle for {}s; exiting", timeout.as_secs());
            shut_down();
        }
    }
}

/// Run the daemon: bind the local endpoint, then serve forwarded commands against
/// the shared session until killed. Never returns under normal operation.
///
/// Binding happens **before** the initial session restore so the daemon is reachable
/// immediately — the (possibly slow, or failing) logon runs in the background and
/// blocks only the first command that actually needs auth, not startup.
pub async fn run_server() -> Result<()> {
    // Don't start a second daemon: if one is already listening, exit cleanly. This
    // also makes the thin-client auto-spawn race-safe (many clients may spawn at
    // once; the losers detect the winner and bow out).
    if transport::connect().await.is_ok() {
        tracing::info!("an aurelia daemon is already running; exiting");
        return Ok(());
    }

    let mut listener = match Listener::bind().await {
        Ok(listener) => listener,
        // Lost a bind race with a concurrently-started daemon — fine if one is up.
        Err(_) if transport::connect().await.is_ok() => {
            tracing::info!("another daemon won the bind race; exiting");
            return Ok(());
        }
        Err(e) => {
            return Err(e).with_context(|| {
                format!("failed to bind daemon endpoint {}", transport::endpoint())
            });
        }
    };
    tracing::info!("aurelia daemon listening on {}", transport::endpoint());

    // Record our version + pid so a differently-versioned thin client can detect the
    // mismatch and restart us instead of forwarding a command our CLI can't parse.
    super::write_daemon_info();

    // Establish the shared session in the background (one logon if a token exists),
    // then keep it alive: the liveness loop reconnects if the socket later dies.
    // Stable ID: respawns replace only us.
    if let Ok(dir) = aurelia::core::config::config_dir() {
        aurelia::steam_client::use_stable_logon_id(&format!(
            "{}|{}",
            dir.display(),
            transport::endpoint()
        ));
    }
    let state = super::init_state();
    tokio::spawn(async move {
        state.ensure_session().await;
        auto_resume_installs().await;
    });
    tokio::spawn(async move { state.liveness_loop().await });

    // Self-restart on binary upgrade so the daemon never serves stale code.
    tokio::spawn(watch_for_upgrade(exe_signature()));
    touch_activity();
    if let Some(timeout) = idle_timeout() {
        tracing::info!("daemon: exits after {}s idle", timeout.as_secs());
        tokio::spawn(watch_for_idle(timeout));
    }

    loop {
        match listener.next().await {
            Ok(stream) => {
                tokio::spawn(async move {
                    if let Err(e) = handle(stream).await {
                        tracing::warn!("daemon: request failed: {e:#}");
                    }
                });
            }
            Err(e) => tracing::warn!("daemon: accept error: {e:#}"),
        }
    }
}

/// Resume interrupted installs when configured.
async fn auto_resume_installs() {
    use aurelia::library::backlog;
    let auto = aurelia::core::config::load_launcher_config()
        .await
        .is_ok_and(|c| c.install_resume == backlog::InstallResume::Auto);
    if !auto {
        return;
    }
    let ids = match backlog::load() {
        Ok(entries) => backlog::auto_resumable(&entries),
        Err(e) => {
            tracing::warn!("daemon: install backlog unreadable: {e:#}");
            return;
        }
    };
    if ids.is_empty() {
        return;
    }
    tracing::info!(
        "daemon: auto-resuming {} interrupted install(s): {ids:?}",
        ids.len()
    );
    // Counts as busy for upgrade restarts.
    let _busy = InflightGuard::new();
    // No client: discard output, stdin at EOF.
    let (out_tx, _) = tokio::sync::mpsc::unbounded_channel();
    let (_, in_rx) = tokio::sync::mpsc::unbounded_channel();
    let resume = crate::commands::resume_entries(&ids, true, true);
    if let Err(e) = crate::output::scoped(OutputCtx::new(out_tx, in_rx, None), resume).await {
        tracing::warn!("daemon: auto-resume: {e:#}");
    }
}

/// The write half of a connection, shared between the exit-frame writer and the
/// background stdout/stderr pump.
use proto::SharedWriter;

/// Read the opening frame and parse the argv header. The first frame on a connection
/// must be the [`proto::C_HEADER`] frame carrying the request's argv.
async fn read_header<R>(reader: &mut R) -> Result<Header>
where
    R: AsyncRead + Unpin,
{
    let (channel, payload) = proto::read_frame(reader)
        .await?
        .context("client closed before sending a request header")?;
    anyhow::ensure!(channel == proto::C_HEADER, "expected a header frame first");
    let header: Header = serde_json::from_slice(&payload).context("malformed request header")?;
    Ok(header)
}

/// Print a config-dir refusal; exit 2.
fn report_refusal(argv: &[String], message: &str) -> i32 {
    use clap::Parser;
    let json = crate::cli::Cli::try_parse_from(argv).is_ok_and(|c| c.json);
    if json {
        crate::commands::print_json_line(&serde_json::json!({
            "error": message,
            "type": aurelia::core::error::ErrorKind::InvalidInput,
        }));
    } else {
        crate::cli_eprintln!("Error: {message}");
    }
    2
}

/// Spawn the pump that forwards captured stdout/stderr chunks to the socket. Ends
/// when `out_tx` (held by the running command's [`OutputCtx`]) is dropped.
fn spawn_output_pump<W>(
    writer: SharedWriter<W>,
    mut out_rx: mpsc::UnboundedReceiver<OutChunk>,
) -> tokio::task::JoinHandle<()>
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        while let Some(chunk) = out_rx.recv().await {
            let ch = match chunk.stream {
                Stream::Stdout => proto::C_STDOUT,
                Stream::Stderr => proto::C_STDERR,
            };
            if proto::send_frame(&writer, ch, &chunk.bytes).await.is_err() {
                break;
            }
        }
    })
}

/// Spawn the pump that forwards client stdin frames to the command's stdin.
///
/// The task returns only when the client's connection actually drops (socket closed
/// or errored) — the caller treats that as a disconnect and aborts the command. A
/// clean stdin-EOF frame instead just drops the stdin sender (signalling EOF to the
/// command) while the connection, and thus disconnect detection, stays live.
fn spawn_stdin_pump<R>(
    mut reader: R,
    in_tx: mpsc::UnboundedSender<Vec<u8>>,
) -> tokio::task::JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut in_tx = Some(in_tx);
        loop {
            match proto::read_frame(&mut reader).await {
                Ok(Some((proto::C_STDIN, data))) => {
                    // Once the command drops its stdin receiver, stop forwarding but
                    // keep watching the socket so a later disconnect is still caught.
                    if let Some(tx) = in_tx.as_ref() {
                        if tx.send(data).is_err() {
                            in_tx = None;
                        }
                    }
                }
                // Client is done sending stdin but stays connected: signal EOF to the
                // command by dropping the sender, and keep watching the socket.
                Ok(Some((proto::C_STDIN_EOF, _))) => in_tx = None,
                // Connection dropped — the client is gone.
                Ok(None) | Err(_) => break,
                Ok(Some(_)) => {} // ignore unexpected channels
            }
        }
    })
}

/// Whether this request also receives the daemon's own output: a verbose `play`,
/// where background session activity (reconnects, liveness) helps diagnose a launch.
fn watches_daemon(argv: &[String]) -> bool {
    use clap::Parser;

    crate::cli::Cli::try_parse_from(argv)
        .is_ok_and(|cli| cli.verbose > 0 && matches!(cli.command, crate::cli::Command::Play { .. }))
}

/// Build the request's output relay: the requester's `-v` count (parsed from its
/// argv) and a sink feeding the captured-output channel.
fn output_relay(
    argv: &[String],
    out_tx: &mpsc::UnboundedSender<OutChunk>,
) -> aurelia::infra::logging::relay::Relay {
    use aurelia::infra::logging::relay::{Relay, RelayStream};
    use clap::Parser;

    let verbosity = crate::cli::Cli::try_parse_from(argv).map_or(0, |cli| cli.verbose);
    let weak = out_tx.downgrade();
    Relay::new(
        verbosity,
        std::sync::Arc::new(move |stream, bytes: &[u8]| {
            if let Some(tx) = weak.upgrade() {
                let stream = match stream {
                    RelayStream::Stdout => Stream::Stdout,
                    RelayStream::Stderr => Stream::Stderr,
                };
                let _ = tx.send(OutChunk {
                    stream,
                    bytes: bytes.to_vec(),
                });
            }
        }),
    )
}

/// Handle one forwarded command: read the header, run it with stdio routed over the
/// socket, then send the exit code.
async fn handle<S>(stream: S) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // Count this request as in-flight so the upgrade watcher won't restart mid-run.
    let _inflight = InflightGuard::new();

    let (mut reader, writer) = proto::split_shared(stream);

    let header = read_header(&mut reader).await?;
    let argv = header.argv;
    // Client path canonicalized here too.
    let client_dir = header.config_dir.as_deref().map(transport::canonical);
    let refusal =
        super::config_dir_mismatch(client_dir.as_deref(), super::own_config_dir().as_deref());

    // Captured stdout/stderr → socket.
    let (out_tx, out_rx) = mpsc::unbounded_channel::<OutChunk>();
    let writer_task = spawn_output_pump(writer.clone(), out_rx);

    // Client stdin frames → the command's stdin.
    let (in_tx, in_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let mut reader_task = spawn_stdin_pump(reader, in_tx);

    // Tracing events and child process output → the same captured streams. The relay
    // holds only a weak sender so a game outliving the request can't keep `out_tx`
    // (and thus `writer_task`) alive.
    let relay = output_relay(&argv, &out_tx);
    // Held until this request ends (including the disconnect early return).
    let _daemon_watch =
        watches_daemon(&argv).then(|| aurelia::infra::logging::relay::watch_daemon(relay.clone()));

    // Run the command with IO routed to this connection. Dropping `ctx` when the
    // scope ends closes `out_tx`, which ends `writer_task`.
    let ctx = OutputCtx::new(out_tx, in_rx, header.cwd);
    let refused = refusal.is_some();
    let run_argv = argv.clone();
    let run = async move {
        match refusal {
            Some(msg) => report_refusal(&run_argv, &msg),
            None => crate::run_argv(run_argv).await,
        }
    };
    let command = crate::output::scoped(ctx, aurelia::infra::logging::relay::scoped(relay, run));
    tokio::pin!(command);

    // Run the command, but if the client's connection drops abort the command
    // instead of leaving it running — a cancelled QR login must actually stop
    // polling Steam.
    let code = tokio::select! {
        code = &mut command => code,
        _ = &mut reader_task => {
            tracing::info!("client disconnected; aborting in-flight command");
            return Ok(());
        }
    };

    // login/logout change the persisted token — adopt/clear the shared session now.
    if !refused {
        super::maybe_refresh_after(&argv).await;
    }

    // Drain all captured output before the exit frame, then stop the stdin pump.
    let _ = writer_task.await;
    reader_task.abort();

    let mut w = writer.lock().await;
    proto::write_frame(&mut *w, proto::C_EXIT, &code.to_be_bytes()).await?;
    let _ = w.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod idle_tests {
    use super::*;

    #[test]
    fn expires_only_when_idle_long_enough() {
        let timeout = Duration::from_secs(600);
        assert!(!idle_expired(1_000, 1_599, timeout, false));
        assert!(idle_expired(1_000, 1_600, timeout, false));
        assert!(!idle_expired(1_000, 9_999, timeout, true));
    }

    #[test]
    fn clock_going_backwards_never_expires() {
        assert!(!idle_expired(2_000, 1_000, Duration::from_secs(60), false));
    }
}
