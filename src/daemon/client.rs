//! Thin-client side: forward this invocation to a running daemon (auto-spawning one
//! if needed) and relay stdio + exit code. Returns `Ok(None)` to mean "no daemon
//! available — run the command locally instead".

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;

use super::transport;
use super::{proto, Header};

/// How long to wait for an auto-spawned daemon to come up before giving up and
/// running locally.
const SPAWN_WAIT: std::time::Duration = std::time::Duration::from_millis(100);
const SPAWN_ATTEMPTS: u32 = 50; // ~5s total

/// Try to run this command via the daemon. `Ok(Some(code))` — handled by the daemon
/// (relayed); `Ok(None)` — no daemon and none could be started, run locally.
pub async fn try_forward() -> Result<Option<i32>> {
    let Some(stream) = connect_or_spawn().await else {
        return Ok(None);
    };
    let argv: Vec<String> = std::env::args().collect();
    forward(stream, argv).await.map(Some)
}

/// Connect to the daemon; if none is listening, spawn one and wait for it.
async fn connect_or_spawn() -> Option<impl AsyncRead + AsyncWrite + Unpin + Send> {
    if let Ok(stream) = transport::connect().await {
        return Some(stream);
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

/// Send the header + our stdin, relay the daemon's stdout/stderr, return its exit code.
async fn forward<S>(stream: S, argv: Vec<String>) -> Result<i32>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, writer) = tokio::io::split(stream);
    let writer = Arc::new(Mutex::new(writer));

    // Header first.
    let header = serde_json::to_vec(&Header { argv })?;
    {
        let mut w = writer.lock().await;
        proto::write_frame(&mut *w, proto::C_HEADER, &header)
            .await
            .context("failed sending request to daemon")?;
    }

    // Pump our stdin → daemon. Runs concurrently; aborted once the command exits, so
    // a command that never reads stdin doesn't block on it.
    let writer_in = writer.clone();
    let stdin_task = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) => {
                    let mut w = writer_in.lock().await;
                    let _ = proto::write_frame(&mut *w, proto::C_STDIN_EOF, &[]).await;
                    break;
                }
                Ok(n) => {
                    let mut w = writer_in.lock().await;
                    if proto::write_frame(&mut *w, proto::C_STDIN, &buf[..n])
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
        match proto::read_frame(&mut reader).await? {
            Some((proto::C_STDOUT, data)) => {
                stdout.write_all(&data).await?;
                stdout.flush().await?;
            }
            Some((proto::C_STDERR, data)) => {
                stderr.write_all(&data).await?;
                stderr.flush().await?;
            }
            Some((proto::C_EXIT, data)) => {
                code = i32::from_be_bytes(data.get(..4).and_then(|b| b.try_into().ok()).unwrap_or([0; 4]));
                break;
            }
            Some(_) => {}
            None => break,
        }
    }
    stdin_task.abort();
    Ok(code)
}
