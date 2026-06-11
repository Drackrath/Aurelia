//! Tiny framed protocol multiplexing stdin/stdout/stderr/exit over one socket.
//!
//! Each frame is `[u8 channel][u32 BE length][payload]`. The client opens with a
//! `HEADER` frame (JSON `{argv}`), then streams `STDIN`/`STDIN_EOF`; the daemon
//! streams `STDOUT`/`STDERR` and finishes with a single `EXIT` frame (4-byte BE
//! exit code).

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

// Client → daemon.
pub const C_HEADER: u8 = 0x01;
pub const C_STDIN: u8 = 0x02;
pub const C_STDIN_EOF: u8 = 0x03;
// Daemon → client.
pub const C_STDOUT: u8 = 0x11;
pub const C_STDERR: u8 = 0x12;
pub const C_EXIT: u8 = 0x13;

/// Write one frame and flush.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    channel: u8,
    payload: &[u8],
) -> std::io::Result<()> {
    w.write_u8(channel).await?;
    w.write_u32(payload.len() as u32).await?;
    if !payload.is_empty() {
        w.write_all(payload).await?;
    }
    w.flush().await?;
    Ok(())
}

/// Read one frame. Returns `Ok(None)` on a clean EOF (peer closed the stream).
pub async fn read_frame<R: AsyncRead + Unpin>(
    r: &mut R,
) -> std::io::Result<Option<(u8, Vec<u8>)>> {
    let channel = match r.read_u8().await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    };
    let len = r.read_u32().await? as usize;
    let mut buf = vec![0u8; len];
    if len > 0 {
        r.read_exact(&mut buf).await?;
    }
    Ok(Some((channel, buf)))
}
