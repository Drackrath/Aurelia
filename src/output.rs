//! CLI output/input routing.
//!
//! In normal (standalone) runs the `cli_*` macros and [`read_line`] go straight to
//! the process's real stdout/stderr/stdin — identical to `println!`/`eprintln!`.
//!
//! Inside the **daemon**, each forwarded command runs within an [`OutputCtx`] scope
//! ([`scoped`]). There, output is captured into a channel (relayed over the socket to
//! the requesting client) and stdin is sourced from the client instead of the
//! daemon's own stdin. This lets one long-lived daemon process execute commands on
//! behalf of many thin-client invocations while each still sees its own streams.

use std::future::Future;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

/// Which output stream a chunk belongs to.
#[derive(Clone, Copy, Debug)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// A chunk of captured output, tagged with its stream.
pub struct OutChunk {
    pub stream: Stream,
    pub bytes: Vec<u8>,
}

/// Per-request IO context installed while the daemon runs a command.
#[derive(Clone)]
pub struct OutputCtx {
    out: mpsc::UnboundedSender<OutChunk>,
    stdin: Arc<Mutex<DaemonStdin>>,
}

impl OutputCtx {
    /// Build a context that sends captured output to `out` and reads stdin lines
    /// from `stdin_rx` (fed with the client's stdin bytes).
    pub fn new(
        out: mpsc::UnboundedSender<OutChunk>,
        stdin_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    ) -> Self {
        Self {
            out,
            stdin: Arc::new(Mutex::new(DaemonStdin {
                rx: stdin_rx,
                buf: Vec::new(),
                eof: false,
            })),
        }
    }
}

tokio::task_local! {
    static CTX: OutputCtx;
}

/// Run `fut` with `ctx` installed as the current output/input context. Any
/// `cli_*` output or [`read_line`] call made while `fut` runs is routed through it.
pub async fn scoped<F: Future>(ctx: OutputCtx, fut: F) -> F::Output {
    CTX.scope(ctx, fut).await
}

/// Write `s` to the given stream — to the captured channel if inside a daemon
/// [`scoped`] context, otherwise to the real process stream.
pub fn write(stream: Stream, s: &str) {
    let routed = CTX.try_with(|ctx| {
        let _ = ctx.out.send(OutChunk {
            stream,
            bytes: s.as_bytes().to_vec(),
        });
    });
    if routed.is_err() {
        use std::io::Write as _;
        match stream {
            // stdout is line-buffered, so the trailing '\n' from cli_println! flushes.
            Stream::Stdout => {
                let _ = std::io::stdout().write_all(s.as_bytes());
            }
            Stream::Stderr => {
                let _ = std::io::stderr().write_all(s.as_bytes());
            }
        }
    }
}

/// Read one line — from the client's forwarded stdin when inside a daemon context,
/// otherwise from the real process stdin. The trailing newline is stripped.
pub async fn read_line() -> std::io::Result<String> {
    if let Ok(stdin) = CTX.try_with(|ctx| ctx.stdin.clone()) {
        let mut guard = stdin.lock().await;
        return guard.read_line().await;
    }
    use tokio::io::AsyncBufReadExt;
    let mut line = String::new();
    let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
    reader.read_line(&mut line).await?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// Buffers the client's forwarded stdin bytes and yields whole lines.
struct DaemonStdin {
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    buf: Vec<u8>,
    eof: bool,
}

impl DaemonStdin {
    async fn read_line(&mut self) -> std::io::Result<String> {
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=pos).collect();
                return Ok(String::from_utf8_lossy(&line)
                    .trim_end_matches(['\n', '\r'])
                    .to_string());
            }
            if self.eof {
                let rest = String::from_utf8_lossy(&self.buf).trim().to_string();
                self.buf.clear();
                return Ok(rest);
            }
            match self.rx.recv().await {
                Some(chunk) => self.buf.extend_from_slice(&chunk),
                None => self.eof = true,
            }
        }
    }
}

/// `println!`-alike that routes through the active output context.
#[macro_export]
macro_rules! cli_println {
    () => { $crate::output::write($crate::output::Stream::Stdout, "\n") };
    ($($arg:tt)*) => {
        $crate::output::write($crate::output::Stream::Stdout, &format!("{}\n", format_args!($($arg)*)))
    };
}

/// `print!`-alike that routes through the active output context.
#[macro_export]
macro_rules! cli_print {
    ($($arg:tt)*) => {
        $crate::output::write($crate::output::Stream::Stdout, &format!("{}", format_args!($($arg)*)))
    };
}

/// `eprintln!`-alike that routes through the active output context.
#[macro_export]
macro_rules! cli_eprintln {
    () => { $crate::output::write($crate::output::Stream::Stderr, "\n") };
    ($($arg:tt)*) => {
        $crate::output::write($crate::output::Stream::Stderr, &format!("{}\n", format_args!($($arg)*)))
    };
}

/// `eprint!`-alike that routes through the active output context.
#[macro_export]
macro_rules! cli_eprint {
    ($($arg:tt)*) => {
        $crate::output::write($crate::output::Stream::Stderr, &format!("{}", format_args!($($arg)*)))
    };
}
