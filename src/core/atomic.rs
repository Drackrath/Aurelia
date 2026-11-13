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
        std::fs::rename(&tmp, path).with_context(|| format!("failed replacing {}", path.display()))
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

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
mod tests {
    use super::*;

    #[test]
    fn replaces_contents_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.json");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["config.json"]);
    }

    #[test]
    fn failed_rename_keeps_old_file_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("dir-not-file");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep"), b"x").unwrap();
        assert!(write_atomic(&target, b"data").is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, b"secret").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
