//! Cloud sync outcome, conflict and UFS types.

use super::*;

#[derive(Debug, Clone)]
pub struct CloudFileEntry {
    pub filename: String,
    pub timestamp: u64,
    pub size: u64,
    pub sha_hash: Option<String>,
}

/// How `sync` resolves a file that has **diverged** — present and different on both
/// the cloud and on disk (by SHA-1), with each side changed since the last sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictPolicy {
    /// Don't touch either copy; collect the divergent files into
    /// [`SyncOutcome::conflicts`] so a caller (or UI) can decide. The default.
    #[default]
    Detect,
    /// Overwrite the local copy with the cloud copy.
    TakeCloud,
    /// Overwrite the cloud copy with the local copy.
    TakeLocal,
}

/// Which way `sync` is allowed to move files. Conflicts are always *detected*
/// regardless of direction; only the non-conflicting transfers are filtered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDirection {
    Down,
    Up,
    Both,
}

/// A file that exists on both sides with different contents, where each side has
/// changed since the last recorded in-sync state — so neither can be picked
/// automatically without risking data loss. Surfaced for a Take-Cloud/Take-Local
/// choice.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncConflict {
    pub filename: String,
    pub local_path: String,
    pub local_hash: String,
    pub local_size: u64,
    pub local_timestamp: u64,
    pub cloud_hash: String,
    pub cloud_size: u64,
    pub cloud_timestamp: u64,
}

/// A cloud file that could not be mapped to any local path, so it was neither
/// downloaded nor compared. Almost always an Auto-Cloud root token this platform
/// has no mapping for (a `%Win*%` root with no Proton prefix, or a token Steam
/// added that this build predates). Reported rather than dropped silently: from the
/// outside a skipped file is indistinguishable from an empty cloud, which reads as
/// "sync worked, my saves are gone".
#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncSkip {
    pub filename: String,
    /// The root token that failed to resolve, when the name carried one.
    pub root_token: Option<String>,
    pub reason: String,
}

/// A file whose transfer was attempted and failed (a timed-out RPC, an HTTP error,
/// an unwritable path). Recorded so one bad file doesn't abort the run: aborting
/// leaves a *partially* restored save set, which is worse than either extreme — the
/// game finds some files and starts as though the save were intact.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncFailure {
    pub filename: String,
    /// Which way the transfer was going: `"down"` or `"up"`.
    pub direction: String,
    pub error: String,
}

/// Result of a [`CloudClient::sync`] run: what moved and what couldn't be resolved.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct SyncOutcome {
    pub downloaded: Vec<String>,
    pub uploaded: Vec<String>,
    pub conflicts: Vec<SyncConflict>,
    pub skipped: Vec<SyncSkip>,
    pub failed: Vec<SyncFailure>,
}

impl SyncOutcome {
    pub fn has_conflicts(&self) -> bool {
        !self.conflicts.is_empty()
    }

    pub fn has_skips(&self) -> bool {
        !self.skipped.is_empty()
    }

    pub fn has_failures(&self) -> bool {
        !self.failed.is_empty()
    }

    /// True when some file did not end up in the state the sync intended — the
    /// transfer counts alone can't be read as success.
    pub fn is_incomplete(&self) -> bool {
        self.has_failures() || self.has_skips()
    }

    pub(super) fn record_failure(
        &mut self,
        filename: &str,
        direction: &str,
        error: &anyhow::Error,
    ) {
        tracing::warn!("cloud sync {direction} failed for '{filename}': {error:#}");
        self.failed.push(SyncFailure {
            filename: filename.to_string(),
            direction: direction.to_string(),
            error: format!("{error:#}"),
        });
    }

    /// The distinct unresolvable root tokens behind [`Self::skipped`], for a summary
    /// that names the actual problem instead of listing every affected file.
    pub fn skipped_tokens(&self) -> Vec<String> {
        let mut tokens: Vec<String> = self
            .skipped
            .iter()
            .filter_map(|s| s.root_token.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        tokens.sort();
        tokens
    }
}

/// One Steam Auto-Cloud save-file rule from an app's `ufs/savefiles` appinfo: which
/// root directory to look under, the sub-path, the filename glob, and whether to
/// recurse. Used to discover local saves that aren't in the cloud yet so they get
/// their first upload.
#[derive(Debug, Clone)]
pub struct UfsSaveSpec {
    pub root: String,
    pub path: String,
    pub pattern: String,
    pub recursive: bool,
}

/// One `ufs/rootoverrides` rule.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UfsRootOverride {
    /// Token being overridden, e.g. `WinAppDataRoaming`.
    pub root: String,
    /// OS name: `Windows`, `Linux`, `MacOS`.
    pub os: String,
    /// `=` (default) or `!=`.
    pub os_compare: String,
    /// Replacement root token.
    pub use_instead: String,
    /// Sub-path appended to the replacement.
    pub add_path: String,
    /// `(find, replace)` applied to paths.
    pub path_transforms: Vec<(String, String)>,
}

impl UfsRootOverride {
    /// Whether it applies to `target_os`.
    pub fn applies_to(&self, target_os: &str) -> bool {
        let same = self.os.is_empty() || self.os.eq_ignore_ascii_case(target_os);
        if self.os_compare.trim() == "!=" {
            !same
        } else {
            same
        }
    }
}

/// An app's UFS appinfo section.
#[derive(Debug, Clone, Default)]
pub struct UfsConfig {
    pub savefiles: Vec<UfsSaveSpec>,
    pub root_overrides: Vec<UfsRootOverride>,
}
