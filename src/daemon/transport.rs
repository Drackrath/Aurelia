//! Cross-platform local IPC endpoint: a Unix domain socket on Unix, a named pipe
//! on Windows. Both stream types implement `AsyncRead + AsyncWrite`, so the server
//! and client code stay generic over the concrete type.
//!
//! The endpoint path can be overridden with `AURELIA_DAEMON_SOCKET` (used by the
//! `--socket` flag) so a driver can isolate its own daemon.

/// Human-readable endpoint (socket path / pipe name), for logging.
pub fn endpoint() -> String {
    imp::endpoint()
}

/// Filesystem path of the daemon's identity marker (version + pid), written by the
/// running daemon and read by thin clients to detect a version mismatch. Kept next to
/// the socket so a custom `AURELIA_DAEMON_SOCKET` gets its own isolated marker.
pub fn version_marker_path() -> std::path::PathBuf {
    imp::version_marker_path()
}

#[cfg(unix)]
mod imp {
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};

    pub fn endpoint() -> String {
        if let Ok(p) = std::env::var("AURELIA_DAEMON_SOCKET") {
            return p;
        }
        let base = std::env::var("XDG_RUNTIME_DIR")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let uid = unsafe { libc::getuid() };
        base.join(format!("aurelia-{uid}.sock"))
            .to_string_lossy()
            .into_owned()
    }

    pub fn version_marker_path() -> PathBuf {
        // Alongside the socket (`aurelia-{uid}.info`), so it inherits any
        // AURELIA_DAEMON_SOCKET override the endpoint uses.
        PathBuf::from(endpoint()).with_extension("info")
    }

    pub struct Listener(UnixListener);

    impl Listener {
        pub async fn bind() -> std::io::Result<Self> {
            let path = endpoint();
            // A stale socket file from a previous run blocks bind(); remove it.
            let _ = std::fs::remove_file(&path);
            if let Some(parent) = std::path::Path::new(&path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            Ok(Self(UnixListener::bind(&path)?))
        }

        pub async fn next(&mut self) -> std::io::Result<UnixStream> {
            let (stream, _addr) = self.0.accept().await?;
            Ok(stream)
        }
    }

    pub async fn connect() -> std::io::Result<UnixStream> {
        UnixStream::connect(endpoint()).await
    }
}

#[cfg(windows)]
mod imp {
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };

    const ERROR_PIPE_BUSY: i32 = 231;

    pub fn endpoint() -> String {
        if let Ok(p) = std::env::var("AURELIA_DAEMON_SOCKET") {
            return p;
        }
        let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".to_string());
        format!(r"\\.\pipe\aurelia-{user}")
    }

    pub fn version_marker_path() -> std::path::PathBuf {
        // A named pipe has no filesystem path, so keep the marker in the temp dir —
        // keyed by the *endpoint name* (sanitized), so an AURELIA_DAEMON_SOCKET
        // override gets its own isolated marker instead of sharing (and clobbering)
        // the default daemon's identity.
        let name: String = endpoint()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        std::env::temp_dir().join(format!("{name}.info"))
    }

    pub struct Listener {
        name: String,
        server: NamedPipeServer,
    }

    impl Listener {
        pub async fn bind() -> std::io::Result<Self> {
            let name = endpoint();
            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&name)?;
            Ok(Self { name, server })
        }

        pub async fn next(&mut self) -> std::io::Result<NamedPipeServer> {
            // Wait for a client, then swap in a fresh instance for the next one and
            // hand the connected instance to the caller.
            self.server.connect().await?;
            let next = ServerOptions::new().create(&self.name)?;
            Ok(std::mem::replace(&mut self.server, next))
        }
    }

    pub async fn connect() -> std::io::Result<NamedPipeClient> {
        let name = endpoint();
        loop {
            match ClientOptions::new().open(&name) {
                Ok(client) => return Ok(client),
                // The pipe exists but every instance is momentarily busy; retry.
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

pub use imp::{connect, Listener};
