//! Crash-safe file replacement.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Per-process temp file counter.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Replace `path` via temp-and-rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_with_mode(path, bytes, false)
}

/// Like [`write_atomic`], owner-only from creation.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    write_with_mode(path, bytes, true)
}

/// Async [`write_atomic`] on the blocking pool.
pub async fn write_atomic_async(path: PathBuf, bytes: Vec<u8>, private: bool) -> Result<()> {
    tokio::task::spawn_blocking(move || write_with_mode(&path, &bytes, private))
        .await
        .context("atomic write task failed")?
}

fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()))
}

fn write_with_mode(path: &Path, bytes: &[u8], private: bool) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
    }
    let tmp = temp_path(path);
    let result = write_temp(&tmp, bytes, private).and_then(|()| {
        if !private {
            keep_metadata(path, &tmp);
        }
        std::fs::rename(&tmp, path).with_context(|| format!("failed replacing {}", path.display()))
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Copy mode and owner, best effort.
#[cfg(unix)]
fn keep_metadata(path: &Path, tmp: &Path) {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    let _ = std::fs::set_permissions(tmp, meta.permissions());
    let _ = std::os::unix::fs::chown(tmp, Some(meta.uid()), Some(meta.gid()));
}

#[cfg(not(unix))]
fn keep_metadata(_path: &Path, _tmp: &Path) {}

fn write_temp(tmp: &Path, bytes: &[u8], private: bool) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options
        .open(tmp)
        .with_context(|| format!("failed creating {}", tmp.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("failed writing {}", tmp.display()))
}

#[cfg(test)]
mod tests;
