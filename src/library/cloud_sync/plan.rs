//! Sync baseline and per-file action planning.

use super::*;

/// One file's last-known **in-sync** fingerprint, persisted per app so the next
/// sync can tell a one-sided change (auto-resolvable) from a true divergence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct BaselineEntry {
    pub(super) hash: String,
    pub(super) timestamp: u64,
    pub(super) size: u64,
}

/// Per-app record of the last state at which cloud and local agreed. Without it a
/// sync can't distinguish "only the local copy changed since we last synced"
/// (a normal upload) from "both copies changed independently" (a real conflict),
/// and would either clobber data or flag a conflict on every routine save.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct SyncBaseline {
    pub(super) files: HashMap<String, BaselineEntry>,
}

impl SyncBaseline {
    pub(super) fn path_for(appid: u32) -> Result<PathBuf> {
        Ok(crate::core::config::config_dir()?
            .join("cloud_sync")
            .join(format!("{appid}.json")))
    }

    pub(super) fn load(appid: u32) -> Self {
        let Ok(path) = Self::path_for(appid) else {
            return Self::default();
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub(super) fn save(&self, appid: u32) {
        let Ok(path) = Self::path_for(appid) else {
            return;
        };
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            tracing::warn!("could not create cloud-sync state dir: {e}");
            return;
        }
        match serde_json::to_string_pretty(self) {
            Ok(raw) => {
                if let Err(e) = crate::core::atomic::write_atomic(&path, raw.as_bytes()) {
                    tracing::warn!("could not persist cloud-sync state {}: {e}", path.display());
                }
            }
            Err(e) => tracing::warn!("could not serialize cloud-sync state: {e}"),
        }
    }
}

/// SHA-1 of a file's bytes as lowercase hex — the same form Steam reports in
/// `file_sha` — for content comparison. `None` if the file can't be read.
pub(super) fn sha1_hex_of_file(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = Sha1::new();
    hasher.update(&bytes);
    Some(hex_lower(&hasher.finalize()))
}

pub(super) fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Cloud SHA hashes arrive as hex strings; normalize both sides before comparing
/// so case/whitespace differences don't read as a divergence.
pub(super) fn hash_eq(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// What `sync` should do with a single file after comparing the local copy, the
/// cloud copy and the recorded baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlannedAction {
    /// Contents already match (or there's nothing to move) — leave both copies.
    Skip,
    /// The cloud copy is the one that moved on — fetch it.
    Download,
    /// The local copy is the one that moved on — push it.
    Upload,
    /// Both copies changed independently — needs a Take-Cloud/Take-Local decision.
    Conflict,
}

/// The on-disk facts about a local save needed to compare it with the cloud.
pub(super) struct LocalInfo {
    pub(super) hash: String,
    pub(super) size: u64,
    pub(super) timestamp: u64,
}

impl LocalInfo {
    pub(super) fn as_baseline(&self) -> BaselineEntry {
        BaselineEntry {
            hash: self.hash.clone(),
            timestamp: self.timestamp,
            size: self.size,
        }
    }
}

/// Read a local file's size, mtime (epoch secs) and content hash. `None` when the
/// path is absent or isn't a regular file (so it counts as "not present locally").
pub(super) fn read_local_info(path: &Path) -> Option<LocalInfo> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let timestamp = meta
        .modified()
        .ok()
        .and_then(crate::core::time::unix_secs)
        .unwrap_or_default();
    let hash = sha1_hex_of_file(path)?;
    Some(LocalInfo {
        hash,
        size: meta.len(),
        timestamp,
    })
}

/// Baseline fingerprint for a just-downloaded cloud file. Prefer Steam's reported
/// `file_sha`; if it's missing, hash the bytes we just wrote so the entry is still
/// usable for the next comparison.
pub(super) fn cloud_baseline(cloud: &CloudFileEntry, local_path: &Path) -> BaselineEntry {
    let hash = cloud
        .sha_hash
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| sha1_hex_of_file(local_path))
        .unwrap_or_default();
    BaselineEntry {
        hash,
        timestamp: cloud.timestamp,
        size: cloud.size,
    }
}

/// Decide what to do with one save given its baseline, local and cloud states.
///
/// The baseline is what lets a one-sided change (auto-resolvable) be told apart
/// from a genuine two-sided divergence. With no baseline yet, two differing copies
/// are treated as a conflict — we can't know which lineage is correct.
pub(super) fn plan_action(
    baseline: Option<&BaselineEntry>,
    local: Option<&LocalInfo>,
    cloud: Option<&CloudFileEntry>,
) -> PlannedAction {
    match (local, cloud) {
        (Some(l), Some(c)) => match c.sha_hash.as_deref() {
            // Identical content: nothing to move regardless of timestamps.
            Some(ch) if hash_eq(&l.hash, ch) => PlannedAction::Skip,
            Some(ch) => {
                let local_changed = baseline.is_none_or(|b| !hash_eq(&b.hash, &l.hash));
                let cloud_changed = baseline.is_none_or(|b| !hash_eq(&b.hash, ch));
                match (local_changed, cloud_changed) {
                    (true, true) => PlannedAction::Conflict,
                    (true, false) => PlannedAction::Upload,
                    (false, true) => PlannedAction::Download,
                    // Neither differs from the baseline, yet the two hashes differ —
                    // impossible, but never overwrite on an inconsistency.
                    (false, false) => PlannedAction::Skip,
                }
            }
            // Cloud gave no hash: fall back to mtime/size; can't detect a conflict.
            None => {
                if l.size == c.size && l.timestamp == c.timestamp {
                    PlannedAction::Skip
                } else if l.timestamp >= c.timestamp {
                    PlannedAction::Upload
                } else {
                    PlannedAction::Download
                }
            }
        },
        // Present on only one side: move it the only way it can go.
        (Some(_), None) => PlannedAction::Upload,
        (None, Some(_)) => PlannedAction::Download,
        (None, None) => PlannedAction::Skip,
    }
}
