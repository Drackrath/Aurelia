//! Persistent install backlog (`install_backlog.json`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::core::config::config_dir;

/// Automatic resumes allowed per entry.
pub const MAX_AUTO_ATTEMPTS: u32 = 3;

/// Why an install is waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BacklogState {
    /// Waiting to be resumed.
    Queued,
    /// User paused it.
    Paused,
    /// User stopped it.
    Aborted,
    /// Ended without finishing.
    Interrupted,
}

impl BacklogState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Paused => "paused",
            Self::Aborted => "aborted",
            Self::Interrupted => "interrupted",
        }
    }
}

/// `config install-resume` modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallResume {
    /// Wait for `install resume`.
    #[default]
    Manual,
    /// Daemon resumes interrupted installs.
    Auto,
}

/// One pending install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BacklogEntry {
    pub app_id: u32,
    #[serde(default)]
    pub name: String,
    pub state: BacklogState,
    /// `windows` or `linux`, as resolved.
    pub platform: String,
    /// Library root it installs into.
    pub library: String,
    /// Partial files, when known.
    #[serde(default)]
    pub install_dir: Option<PathBuf>,
    /// DLC content lives in the base game.
    #[serde(default)]
    pub is_dlc: bool,
    #[serde(default)]
    pub restart_steam: bool,
    #[serde(default)]
    pub downloaded_bytes: u64,
    #[serde(default)]
    pub total_bytes: u64,
    /// Last failure, for `interrupted`.
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub error_type: Option<String>,
    #[serde(default)]
    pub auto_attempts: u32,
    pub added_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct BacklogFile {
    version: u32,
    entries: Vec<BacklogEntry>,
}

/// Serializes read-modify-write in-process.
static LOCK: Mutex<()> = Mutex::new(());

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `<config>/install_backlog.json`.
pub fn backlog_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("install_backlog.json"))
}

/// All entries, oldest first.
pub fn load() -> Result<Vec<BacklogEntry>> {
    load_from(&backlog_path()?)
}

/// Apply `change` and save atomically.
pub fn update<T>(change: impl FnOnce(&mut Vec<BacklogEntry>) -> T) -> Result<T> {
    update_at(&backlog_path()?, change)
}

/// The entry for `app_id`.
pub fn get(app_id: u32) -> Result<Option<BacklogEntry>> {
    Ok(load()?.into_iter().find(|e| e.app_id == app_id))
}

/// Insert or replace by app id.
pub fn upsert(entry: BacklogEntry) -> Result<()> {
    update(|entries| upsert_in(entries, entry))
}

/// Drop `app_id`; whether it existed.
pub fn remove(app_id: u32) -> Result<bool> {
    update(|entries| {
        let before = entries.len();
        entries.retain(|e| e.app_id != app_id);
        entries.len() != before
    })
}

/// Count an auto-resume before it runs.
pub fn bump_auto_attempts(app_id: u32) -> Result<Option<u32>> {
    update(|entries| bump_in(entries, app_id))
}

pub(crate) fn bump_in(entries: &mut [BacklogEntry], app_id: u32) -> Option<u32> {
    let entry = entries.iter_mut().find(|e| e.app_id == app_id)?;
    entry.auto_attempts += 1;
    entry.updated_at = now();
    Some(entry.auto_attempts)
}

/// Set state; whether `app_id` exists.
pub fn set_state(app_id: u32, state: BacklogState) -> Result<bool> {
    update(|entries| {
        let Some(entry) = entries.iter_mut().find(|e| e.app_id == app_id) else {
            return false;
        };
        entry.state = state;
        entry.updated_at = now();
        true
    })
}

pub(crate) fn upsert_in(entries: &mut Vec<BacklogEntry>, entry: BacklogEntry) {
    match entries.iter_mut().find(|e| e.app_id == entry.app_id) {
        Some(existing) => {
            // Keep the original add time.
            let added_at = existing.added_at;
            *existing = BacklogEntry { added_at, ..entry };
        }
        None => entries.push(entry),
    }
}

/// Entries the daemon may auto-resume.
pub fn auto_resumable(entries: &[BacklogEntry]) -> Vec<u32> {
    entries
        .iter()
        .filter(|e| e.state == BacklogState::Interrupted && e.auto_attempts < MAX_AUTO_ATTEMPTS)
        .map(|e| e.app_id)
        .collect()
}

pub(crate) fn load_from(path: &Path) -> Result<Vec<BacklogEntry>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("failed reading {}", path.display())),
    };
    let file: BacklogFile =
        serde_json::from_str(&raw).with_context(|| format!("failed parsing {}", path.display()))?;
    Ok(file.entries)
}

pub(crate) fn update_at<T>(
    path: &Path,
    change: impl FnOnce(&mut Vec<BacklogEntry>) -> T,
) -> Result<T> {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut entries = load_from(path)?;
    let out = change(&mut entries);
    save_to(path, &entries)?;
    Ok(out)
}

/// Write via temp file and rename.
fn save_to(path: &Path, entries: &[BacklogEntry]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
    }
    let file = BacklogFile {
        version: 1,
        entries: entries.to_vec(),
    };
    let body = serde_json::to_string_pretty(&file)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).with_context(|| format!("failed writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("failed replacing {}", path.display()))?;
    Ok(())
}

/// Folder `forget --delete-files` may remove.
pub fn deletable_dir(entry: &BacklogEntry) -> Option<PathBuf> {
    // A DLC's folder is the base game's.
    if entry.is_dlc {
        return None;
    }
    let dir = entry.install_dir.as_ref()?;
    let common = Path::new(&entry.library).join("steamapps").join("common");
    let name = dir.file_name()?.to_str()?;
    if dir.parent() != Some(common.as_path()) || !crate::core::utils::is_safe_dir_name(name) {
        return None;
    }
    let manifest = Path::new(&entry.library)
        .join("steamapps")
        .join(format!("appmanifest_{}.acf", entry.app_id));
    // A finished install owns it.
    if manifest.exists() {
        return None;
    }
    Some(dir.clone())
}

#[cfg(test)]
#[path = "backlog_tests.rs"]
mod tests;
