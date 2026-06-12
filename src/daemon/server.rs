//! Daemon server: accept forwarded commands and run them against the shared session.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex};

use super::transport::{self, Listener};
use super::{proto, Header};
use crate::output::{OutChunk, OutputCtx, Stream};

/// Number of forwarded commands currently running, so the upgrade watcher only
/// restarts the daemon while it is idle (never mid-request).
static INFLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Increments [`INFLIGHT`] for its lifetime; decrements on drop (covers `?` early
/// returns in [`handle`]).
struct InflightGuard;

impl InflightGuard {
    fn new() -> Self {
        INFLIGHT.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        INFLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// `(mtime, len)` of the running executable, used to detect an on-disk upgrade.
fn exe_signature() -> Option<(SystemTime, u64)> {
    let exe = std::env::current_exe().ok()?;
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
        if let Some(current) = exe_signature() {
            if current != startup && INFLIGHT.load(Ordering::SeqCst) == 0 {
                tracing::info!(
                    "daemon: binary changed on disk; exiting so the next command starts a \
                     fresh daemon"
                );
                std::process::exit(0);
            }
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
        Err(e) => {
            // Lost a bind race with a concurrently-started daemon — fine if one is up.
            if transport::connect().await.is_ok() {
                tracing::info!("another daemon won the bind race; exiting");
                return Ok(());
            }
            return Err(e)
                .with_context(|| format!("failed to bind daemon endpoint {}", transport::endpoint()));
        }
    };
    tracing::info!("aurelia daemon listening on {}", transport::endpoint());

    // Establish the shared session in the background (one logon if a token exists),
    // then keep it alive: the liveness loop reconnects if the socket later dies.
    let state = super::init_state();
    tokio::spawn(async move { state.ensure_session().await });
    tokio::spawn(async move { state.liveness_loop().await });

    // Self-restart on binary upgrade so the daemon never serves stale code.
    tokio::spawn(watch_for_upgrade(exe_signature()));

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

/// Handle one forwarded command: read the header, run it with stdio routed over the
/// socket, then send the exit code.
async fn handle<S>(stream: S) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // Count this request as in-flight so the upgrade watcher won't restart mid-run.
    let _inflight = InflightGuard::new();

    let (mut reader, writer) = tokio::io::split(stream);
    let writer = Arc::new(Mutex::new(writer));

    // First frame must be the argv header.
    let (channel, payload) = proto::read_frame(&mut reader)
        .await?
        .context("client closed before sending a request header")?;
    anyhow::ensure!(channel == proto::C_HEADER, "expected a header frame first");
    let header: Header = serde_json::from_slice(&payload).context("malformed request header")?;
    let argv = header.argv;

    // Captured stdout/stderr → socket.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<OutChunk>();
    let writer_out = writer.clone();
    let writer_task = tokio::spawn(async move {
        while let Some(chunk) = out_rx.recv().await {
            let ch = match chunk.stream {
                Stream::Stdout => proto::C_STDOUT,
                Stream::Stderr => proto::C_STDERR,
            };
            let mut w = writer_out.lock().await;
            if proto::write_frame(&mut *w, ch, &chunk.bytes).await.is_err() {
                break;
            }
        }
    });

    // Client stdin frames → the command's stdin.
    let (in_tx, in_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let reader_task = tokio::spawn(async move {
        loop {
            match proto::read_frame(&mut reader).await {
                Ok(Some((proto::C_STDIN, data))) => {
                    if in_tx.send(data).is_err() {
                        break;
                    }
                }
                Ok(Some((proto::C_STDIN_EOF, _))) | Ok(None) => break,
                Ok(Some(_)) => {} // ignore unexpected channels
                Err(_) => break,
            }
        }
        // Dropping `in_tx` here signals stdin EOF to the command.
    });

    // Run the command with IO routed to this connection. Dropping `ctx` when the
    // scope ends closes `out_tx`, which ends `writer_task`.
    let ctx = OutputCtx::new(out_tx, in_rx);
    let code = crate::output::scoped(ctx, crate::run_argv(argv.clone())).await;

    // login/logout change the persisted token — adopt/clear the shared session now.
    super::maybe_refresh_after(&argv).await;

    // Drain all captured output before the exit frame, then stop the stdin pump.
    let _ = writer_task.await;
    reader_task.abort();

    let mut w = writer.lock().await;
    proto::write_frame(&mut *w, proto::C_EXIT, &code.to_be_bytes()).await?;
    let _ = w.shutdown().await;
    Ok(())
}
