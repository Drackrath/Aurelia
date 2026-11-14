//! Daemon log file, size-rotated.
//!
//! `logs/daemon.log` plus two older files.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Rotate past this size.
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;
/// Older files kept beside it.
pub const KEEP: usize = 2;

/// Steam crates capped: may hold tokens.
const QUIET: &str =
    "steam_vent=warn,steam_vent_proto=warn,steam_vent_core=warn,aurelia_steam_vent_chat=warn";

/// Owner-only: SteamIDs and game activity.
const MODE: u32 = 0o600;

/// File filter for the daemon's verbosity.
pub fn directives(verbosity: u8) -> String {
    let base = if verbosity >= 1 { "debug" } else { "info" };
    format!("{base},{QUIET}")
}

/// The daemon's file layer.
pub fn file_layer<S>(writer: RotatingFile, verbosity: u8) -> impl tracing_subscriber::Layer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    use tracing_subscriber::Layer;
    tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .log_internal_errors(false)
        .with_filter(tracing_subscriber::EnvFilter::new(directives(verbosity)))
}

/// Tightens older files to owner-only.
fn restrict(path: &Path) {
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(MODE));
}

/// Where the daemon log lives.
pub fn daemon_log_path() -> Option<PathBuf> {
    crate::core::config::config_dir()
        .ok()
        .map(|d| d.join("logs").join("daemon.log"))
}

/// `daemon.log.N` for rotation slot `n`.
pub fn rotated(path: &Path, n: usize) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{n}"));
    PathBuf::from(name)
}

/// Shifts files up; oldest drops off.
pub fn rotate(path: &Path, keep: usize) {
    if keep == 0 {
        let _ = std::fs::remove_file(path);
        return;
    }
    let _ = std::fs::remove_file(rotated(path, keep));
    for n in (1..keep).rev() {
        let _ = std::fs::rename(rotated(path, n), rotated(path, n + 1));
    }
    let _ = std::fs::rename(path, rotated(path, 1));
    for n in 1..=keep {
        let old = rotated(path, n);
        if old.exists() {
            restrict(&old);
        }
    }
}

struct Inner {
    path: PathBuf,
    max: u64,
    keep: usize,
    file: Option<File>,
    size: u64,
}

impl Inner {
    fn open(&mut self) -> Option<&mut File> {
        if self.file.is_none() {
            if let Some(dir) = self.path.parent() {
                let _ = std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(dir);
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .mode(MODE)
                .open(&self.path)
                .ok()?;
            // Files from older versions too.
            let _ = file.set_permissions(std::fs::Permissions::from_mode(MODE));
            self.size = file.metadata().map_or(0, |m| m.len());
            self.file = Some(file);
        }
        self.file.as_mut()
    }

    fn write(&mut self, buf: &[u8]) {
        if self.size > 0 && self.size + buf.len() as u64 > self.max {
            self.file = None;
            rotate(&self.path, self.keep);
        }
        let Some(file) = self.open() else { return };
        if file.write_all(buf).is_ok() {
            self.size += buf.len() as u64;
        }
    }
}

/// Shared, size-capped log file.
#[derive(Clone)]
pub struct RotatingFile(Arc<Mutex<Inner>>);

impl RotatingFile {
    pub fn new(path: PathBuf, max: u64, keep: usize) -> Self {
        Self(Arc::new(Mutex::new(Inner {
            path,
            max,
            keep,
            file: None,
            size: 0,
        })))
    }
}

/// One event's writer handle.
pub struct LineWriter(Arc<Mutex<Inner>>);

impl Write for LineWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).write(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RotatingFile {
    type Writer = LineWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LineWriter(Arc::clone(&self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(file: &RotatingFile, text: &str) {
        use tracing_subscriber::fmt::MakeWriter;
        file.make_writer().write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn rotates_at_the_cap_and_keeps_two() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/daemon.log");
        let file = RotatingFile::new(path.clone(), 10, 2);
        for line in ["aaaaaaaa\n", "bbbbbbbb\n", "cccccccc\n", "dddddddd\n"] {
            write(&file, line);
        }
        let read = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
        assert_eq!(read(&path), "dddddddd\n");
        assert_eq!(read(&rotated(&path, 1)), "cccccccc\n");
        assert_eq!(read(&rotated(&path, 2)), "bbbbbbbb\n");
        assert!(!rotated(&path, 3).exists(), "oldest dropped");
    }

    #[test]
    fn appends_until_full_and_resumes_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.log");
        std::fs::write(&path, "old\n").unwrap();
        let file = RotatingFile::new(path.clone(), 10, 1);
        write(&file, "new\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old\nnew\n");
        // 8 + 4 > 10: rotates.
        write(&file, "xyz\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "xyz\n");
        assert_eq!(
            std::fs::read_to_string(rotated(&path, 1)).unwrap(),
            "old\nnew\n"
        );
    }

    #[test]
    fn oversized_line_still_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.log");
        let file = RotatingFile::new(path.clone(), 4, 2);
        write(&file, "0123456789\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "0123456789\n");
    }

    #[test]
    fn steam_crates_capped_at_every_verbosity() {
        for v in [0, 1, 3] {
            let d = directives(v);
            for target in ["steam_vent", "steam_vent_core", "aurelia_steam_vent_chat"] {
                assert!(d.contains(&format!("{target}=warn")), "{d}");
            }
        }
        assert!(directives(0).starts_with("info"));
    }

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn log_and_rotated_files_are_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/daemon.log");
        // Old 0644 file is tightened.
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(rotated(&path, 2), "older\n").unwrap();
        std::fs::set_permissions(rotated(&path, 2), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        std::fs::write(&path, "old\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let file = RotatingFile::new(path.clone(), 10, 3);
        // Appends; open tightens it.
        write(&file, "aaaaaaaa\n");
        assert_eq!(mode(&path), 0o600);
        // Rotates: log to .1, older to .3.
        write(&file, "bbbbbbbb\n");
        for p in [path.clone(), rotated(&path, 1), rotated(&path, 3)] {
            assert_eq!(mode(&p), 0o600, "{}", p.display());
        }
    }

    #[test]
    fn steam_vent_info_never_reaches_the_file() {
        use tracing_subscriber::layer::SubscriberExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.log");
        let file = RotatingFile::new(path.clone(), MAX_BYTES, KEEP);
        for v in [0, 1] {
            let sub = tracing_subscriber::registry().with(file_layer(file.clone(), v));
            tracing::subscriber::with_default(sub, || {
                tracing::info!(target: "steam_vent::connection", "vent-secret");
                tracing::info!(target: "steam_vent_core::message", "core-secret");
                tracing::info!(target: "aurelia_steam_vent_chat", "chat-secret");
                tracing::warn!(target: "steam_vent::connection", "vent-warning");
                tracing::info!(target: "aurelia::daemon", "daemon-info");
            });
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("secret"), "{text}");
        assert_eq!(text.matches("vent-warning").count(), 2, "{text}");
        assert_eq!(text.matches("daemon-info").count(), 2, "{text}");
    }
}
