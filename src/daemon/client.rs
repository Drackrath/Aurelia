//! Thin-client side: forward this invocation to a running daemon (auto-spawning one
//! if needed) and relay stdio + exit code. Returns `Ok(None)` to mean "no daemon
//! available — run the command locally instead".


use anyhow::{Context, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::transport;
use super::{proto, Header};

/// How long to wait for an auto-spawned daemon to come up before giving up and
/// running locally.
const SPAWN_WAIT: std::time::Duration = std::time::Duration::from_millis(100);
const SPAWN_ATTEMPTS: u32 = 50; // ~5s total

/// Outcome of forwarding to the daemon.
pub enum Forwarded {
    /// Daemon ran it; its exit code.
    Done(i32),
    /// Our stdio died mid-relay; parent gone.
    OutputClosed,
    /// No daemon reachable; run locally.
    Unavailable,
}

/// Run this command via the daemon.
pub async fn try_forward() -> Result<Forwarded> {
    let Some(stream) = connect_or_spawn().await else {
        return Ok(Forwarded::Unavailable);
    };
    let argv: Vec<String> = std::env::args().collect();
    forward(stream, argv).await
}

/// Outcome of a session-password hand-off.
pub enum Unlocked {
    /// Daemon accepted the password.
    Accepted,
    /// Daemon rejected it (message relayed).
    Rejected(String),
    /// No daemon reachable.
    Unavailable,
}

/// Hand the session password to the daemon.
///
/// Spawns one if none is running, so the very next forwarded command can
/// already decrypt the session. Any daemon-side messages are collected
/// (not printed) so the caller decides how to surface them.
pub async fn send_session_password(password: &str) -> Result<Unlocked> {
    let Some(stream) = connect_or_spawn().await else {
        return Ok(Unlocked::Unavailable);
    };
    let (mut reader, writer) = proto::split_shared(stream);
    proto::send_frame(&writer, proto::C_UNLOCK, password.as_bytes())
        .await
        .context("failed sending session password to daemon")?;

    let mut messages = String::new();
    let mut code = 0;
    loop {
        match proto::read_frame(&mut reader).await? {
            Some((proto::C_STDOUT | proto::C_STDERR, data)) => {
                messages.push_str(&String::from_utf8_lossy(&data));
            }
            Some((proto::C_EXIT, data)) => {
                code = i32::from_be_bytes(
                    data.get(..4).and_then(|b| b.try_into().ok()).unwrap_or([0; 4]),
                );
                break;
            }
            Some(_) => {}
            None => break,
        }
    }
    let messages = messages.trim().to_string();
    if code == 0 {
        if !messages.is_empty() {
            tracing::warn!("daemon unlock: {messages}");
        }
        Ok(Unlocked::Accepted)
    } else {
        Ok(Unlocked::Rejected(messages))
    }
}

/// Connect to the daemon; if none is listening, spawn one and wait for it.
///
/// A daemon left over from a previous `aurelia` build parses forwarded commands with its
/// own (stale) CLI, so it rejects newly added subcommands with "unrecognized subcommand".
/// So when an existing daemon is a different version from this binary, stop it and start a
/// fresh one before forwarding.
async fn connect_or_spawn() -> Option<impl AsyncRead + AsyncWrite + Unpin + Send> {
    if let Ok(stream) = transport::connect().await {
        let current = env!("CARGO_PKG_VERSION");
        let info = super::read_daemon_info();
        if !daemon_needs_restart(info.as_ref(), current) {
            return Some(stream);
        }
        // Version mismatch, or an old daemon predating the marker (absent -> "unknown").
        drop(stream);
        let reported = info.as_ref().map_or("unknown", |i| i.version.as_str());
        tracing::info!(
            "restarting aurelia daemon: it is running v{reported} but this binary is v{current}"
        );
        restart_daemon(info.as_ref().map(|i| i.pid));

        // Wait (bounded) for the stopped daemon to release the socket, so the replacement
        // we spawn below doesn't bow out to it and we don't reconnect to the very daemon
        // we just stopped.
        for _ in 0..SPAWN_ATTEMPTS {
            if transport::connect().await.is_err() {
                break;
            }
            tokio::time::sleep(SPAWN_WAIT).await;
        }
    }

    // Never auto-spawn when the driver manages the daemon lifecycle itself
    // (AURELIA_NO_SPAWN, set e.g. by Heroic): some driver spawn chains — like
    // `powershell Start-Process -Wait` — wait on the whole descendant tree, so
    // a daemon spawned from this client process would keep that wait blocked
    // for the daemon's lifetime. Run the command locally instead.
    if std::env::var_os("AURELIA_NO_SPAWN").is_some_and(|v| !v.is_empty()) {
        return None;
    }

    spawn_daemon().ok()?;
    for _ in 0..SPAWN_ATTEMPTS {
        tokio::time::sleep(SPAWN_WAIT).await;
        if let Ok(stream) = transport::connect().await {
            return Some(stream);
        }
    }
    None
}

/// Whether an existing daemon described by `info` must be restarted before use.
///
/// True when its version differs from `current`, or the marker is absent/unparseable
/// (`info == None`) — a daemon predating this marker can't be trusted to parse newer
/// commands either, so treat "unknown" as a mismatch. The same applies to the
/// binary identity: a rebuild at an unchanged crate version must also restart the
/// daemon, or it keeps serving stale code.
fn daemon_needs_restart(info: Option<&super::DaemonInfo>, current: &str) -> bool {
    let Some(info) = info else { return true };
    if info.version != current {
        return true;
    }
    match (&info.build_id, super::current_build_id()) {
        (Some(daemon), Some(client)) => *daemon != client,
        // Old marker without identity: conservative restart.
        (None, _) => true,
        // Own identity unreadable: don't restart-loop on it.
        (_, None) => false,
    }
}

/// Stop a stale-version daemon so the next connect spawns a fresh one. `pid` is the
/// daemon's own pid from its marker when available — kill exactly that process. Without a
/// marker (a daemon predating this mechanism) fall back to stopping every session daemon,
/// since we can't otherwise identify which process owns the socket.
fn restart_daemon(pid: Option<u32>) {
    let killed = match pid {
        Some(pid) => crate::proc_admin::kill_pids(&[pid]),
        None => {
            let daemons: Vec<u32> = crate::proc_admin::find_aurelia_processes()
                .into_iter()
                .filter(|p| p.is_daemon)
                .map(|p| p.pid)
                .collect();
            crate::proc_admin::kill_pids(&daemons)
        }
    };
    tracing::info!("stopped {killed} stale aurelia daemon(s)");
    super::clear_daemon_info();
}

/// Launch a detached `aurelia daemon` process.
fn spawn_daemon() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    detach(&mut cmd);
    cmd.spawn()?;
    Ok(())
}

#[cfg(unix)]
fn detach(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // New process group so the daemon outlives the spawning shell / Heroic process.
    cmd.process_group(0);
}

#[cfg(windows)]
fn detach(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    // Windows spawns with bInheritHandles=TRUE, so a detached child inherits every
    // inheritable handle we hold — including the stdout/stderr pipes our own parent
    // (e.g. Heroic) handed us. The long-lived daemon would then keep those pipes
    // open forever and the parent would never see EOF. Clear the inherit flag on our
    // std handles before spawning so the daemon can't capture them.
    clear_std_handle_inheritance();
}

#[cfg(windows)]
fn clear_std_handle_inheritance() {
    use windows_sys::Win32::Foundation::{
        SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: GetStdHandle/SetHandleInformation are plain Win32 calls with no
        // memory-safety preconditions; we only act on a valid, non-null handle.
        unsafe {
            let handle = GetStdHandle(id);
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                // dwFlags = 0 clears HANDLE_FLAG_INHERIT for this handle.
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

/// Write a relayed output chunk to a local stream and flush it immediately, so the
/// daemon's stdout/stderr appears with the same interleaving the user would see when
/// running locally.
async fn write_and_flush<W: AsyncWrite + Unpin>(w: &mut W, data: &[u8]) -> std::io::Result<()> {
    w.write_all(data).await?;
    w.flush().await
}

/// Did our local output stream close?
fn is_output_closed(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof
    )
}

/// Send the header + our stdin, relay the daemon's stdout/stderr, return its exit code.
async fn forward<S>(stream: S, argv: Vec<String>) -> Result<Forwarded>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, writer) = proto::split_shared(stream);

    // Header first.
    let header = serde_json::to_vec(&Header { argv })?;
    proto::send_frame(&writer, proto::C_HEADER, &header)
        .await
        .context("failed sending request to daemon")?;

    // Pump our stdin → daemon. Runs concurrently; aborted once the command exits, so
    // a command that never reads stdin doesn't block on it.
    let writer_in = writer.clone();
    let stdin_task = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) => {
                    let _ = proto::send_frame(&writer_in, proto::C_STDIN_EOF, &[]).await;
                    break;
                }
                Ok(n) => {
                    if proto::send_frame(&writer_in, proto::C_STDIN, &buf[..n])
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Relay daemon output until the exit frame.
    let mut stdout = tokio::io::stdout();
    let mut stderr = tokio::io::stderr();
    let mut code = 0;
    loop {
        // Broken pipe means the parent left.
        let relayed = match proto::read_frame(&mut reader).await? {
            Some((proto::C_STDOUT, data)) => write_and_flush(&mut stdout, &data).await,
            Some((proto::C_STDERR, data)) => write_and_flush(&mut stderr, &data).await,
            Some((proto::C_EXIT, data)) => {
                code = i32::from_be_bytes(data.get(..4).and_then(|b| b.try_into().ok()).unwrap_or([0; 4]));
                break;
            }
            Some(_) => Ok(()),
            None => break,
        };
        if let Err(e) = relayed {
            stdin_task.abort();
            if is_output_closed(&e) {
                return Ok(Forwarded::OutputClosed);
            }
            return Err(e).context("failed relaying daemon output");
        }
    }
    stdin_task.abort();
    Ok(Forwarded::Done(code))
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
