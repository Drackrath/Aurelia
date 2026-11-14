//! Daemon log file, size-rotated.
//!
//! `logs/daemon.log` plus two older files.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Rotate past this size.
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;
/// Older files kept beside it.
pub const KEEP: usize = 2;

/// Default file directives.
///
/// steam-vent stays quiet: may hold tokens.
pub const DIRECTIVES: &str = "info,steam_vent=warn,steam_vent_proto=warn";

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
                let _ = std::fs::create_dir_all(dir);
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .ok()?;
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
    fn secrets_stay_out_by_default() {
        assert!(DIRECTIVES.contains("steam_vent=warn"));
        assert!(DIRECTIVES.starts_with("info"));
    }
}
