use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use sha1::{Digest, Sha1};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use steam_vent::connection::Connection;
use steam_vent_proto::steammessages_cloud_steamclient::{
    CCloud_ClientBeginFileUpload_Request, CCloud_ClientBeginFileUpload_Response,
    CCloud_ClientCommitFileUpload_Request, CCloud_ClientCommitFileUpload_Response,
    CCloud_ClientFileDownload_Request, CCloud_ClientFileDownload_Response,
    CCloud_EnumerateUserFiles_Request, CCloud_EnumerateUserFiles_Response,
};
use steam_vent::ConnectionTrait;

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

/// Result of a [`CloudClient::sync`] run: what moved and what couldn't be resolved.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct SyncOutcome {
    pub downloaded: Vec<String>,
    pub uploaded: Vec<String>,
    pub conflicts: Vec<SyncConflict>,
}

impl SyncOutcome {
    pub fn has_conflicts(&self) -> bool {
        !self.conflicts.is_empty()
    }
}

/// One file's last-known **in-sync** fingerprint, persisted per app so the next
/// sync can tell a one-sided change (auto-resolvable) from a true divergence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct BaselineEntry {
    hash: String,
    timestamp: u64,
    size: u64,
}

/// Per-app record of the last state at which cloud and local agreed. Without it a
/// sync can't distinguish "only the local copy changed since we last synced"
/// (a normal upload) from "both copies changed independently" (a real conflict),
/// and would either clobber data or flag a conflict on every routine save.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct SyncBaseline {
    files: HashMap<String, BaselineEntry>,
}

impl SyncBaseline {
    fn path_for(appid: u32) -> Result<PathBuf> {
        Ok(crate::core::config::config_dir()?
            .join("cloud_sync")
            .join(format!("{appid}.json")))
    }

    fn load(appid: u32) -> Self {
        let Ok(path) = Self::path_for(appid) else {
            return Self::default();
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save(&self, appid: u32) {
        let Ok(path) = Self::path_for(appid) else {
            return;
        };
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("could not create cloud-sync state dir: {e}");
                return;
            }
        }
        match serde_json::to_string_pretty(self) {
            Ok(raw) => {
                if let Err(e) = std::fs::write(&path, raw) {
                    tracing::warn!("could not persist cloud-sync state {}: {e}", path.display());
                }
            }
            Err(e) => tracing::warn!("could not serialize cloud-sync state: {e}"),
        }
    }
}

/// SHA-1 of a file's bytes as lowercase hex — the same form Steam reports in
/// `file_sha` — for content comparison. `None` if the file can't be read.
fn sha1_hex_of_file(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = Sha1::new();
    hasher.update(&bytes);
    Some(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Cloud SHA hashes arrive as hex strings; normalize both sides before comparing
/// so case/whitespace differences don't read as a divergence.
fn hash_eq(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
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

/// Translates Steam Cloud filenames to real on-disk paths.
///
/// Steam Auto-Cloud filenames embed the real save location as a leading root token,
/// e.g. `%WinAppDataLocalLow%SadSocket/9Kings/save.json`. That token must be mapped
/// to the actual OS directory the game reads/writes — otherwise saves land in a
/// phantom folder under `userdata/.../<appid>/` and Steam always reports a
/// cloud/local mismatch. Classic ISteamRemoteStorage filenames carry no token and
/// live under `remote_root` (`userdata/<accountid>/<appid>/remote`).
pub struct CloudPathResolver {
    remote_root: PathBuf,
    install_dir: Option<PathBuf>,
}

impl CloudPathResolver {
    pub fn new(remote_root: PathBuf, install_dir: Option<PathBuf>) -> Self {
        Self {
            remote_root,
            install_dir,
        }
    }

    /// Map a cloud filename to its real local path, resolving any `%RootToken%`
    /// prefix. Errors only when a token is present but unknown on this platform.
    pub fn resolve(&self, filename: &str) -> Result<PathBuf> {
        match split_root_token(filename) {
            Some((token, rest)) => {
                let base = resolve_root_token(token, self.install_dir.as_deref())
                    .with_context(|| {
                        format!("unsupported Steam Cloud root token '%{token}%' on this platform")
                    })?;
                Ok(join_relative(&base, rest))
            }
            None => Ok(join_relative(&self.remote_root, filename)),
        }
    }

    /// Resolve a bare root token (e.g. `WinAppDataLocalLow`) to its OS directory, or
    /// `None` if it isn't applicable on this platform (e.g. a `%Linux*%` root on
    /// Windows). Used by UFS save discovery.
    pub fn resolve_root(&self, root_token: &str) -> Option<PathBuf> {
        resolve_root_token(root_token, self.install_dir.as_deref())
    }
}

/// Discover local save files matching the app's UFS `savefiles` rules and pair each
/// with the cloud filename Steam would store it under (`%Root%<sub-path>`). Lets
/// `sync_up` upload brand-new saves that aren't in the cloud yet. Rules whose root
/// doesn't apply on this platform are skipped.
fn discover_local_saves(
    specs: &[UfsSaveSpec],
    resolver: &CloudPathResolver,
) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for spec in specs {
        let Some(base) = resolver.resolve_root(&spec.root) else {
            continue;
        };
        let search_root = join_relative(&base, &spec.path);
        if !search_root.is_dir() {
            continue;
        }

        let walker = walkdir::WalkDir::new(&search_root).min_depth(1);
        let walker = if spec.recursive {
            walker
        } else {
            walker.max_depth(1)
        };
        for entry in walker
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_file())
        {
            let name = entry.file_name().to_string_lossy();
            if !glob_matches(&spec.pattern, &name) {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&base) else {
                continue;
            };
            let cloud_name = format!("%{}%{}", spec.root, rel.to_string_lossy().replace('\\', "/"));
            out.push((cloud_name, entry.path().to_path_buf()));
        }
    }
    out
}

/// Match a Steam UFS filename glob (`*`, `?`) against a filename, case-insensitively
/// (Windows save files are matched case-insensitively). An empty/`*` pattern matches
/// everything; an invalid pattern is treated as a match rather than dropping files.
fn glob_matches(pattern: &str, name: &str) -> bool {
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    let mut re = String::from("(?i)^");
    for ch in pattern.chars() {
        match ch {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    regex::Regex::new(&re)
        .map(|r| r.is_match(name))
        .unwrap_or(true)
}

/// Split a leading `%Token%` off an Auto-Cloud filename, returning `(token, rest)`.
/// Returns `None` for classic (token-less) remote-storage filenames.
fn split_root_token(filename: &str) -> Option<(&str, &str)> {
    let after = filename.strip_prefix('%')?;
    let end = after.find('%')?;
    Some((&after[..end], &after[end + 1..]))
}

/// Join a `/`- or `\`-separated cloud-relative path onto `base`, skipping empty,
/// `.` and `..` components so a crafted filename can't escape the base directory.
fn join_relative(base: &Path, rel: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    for comp in rel.split(['/', '\\']) {
        if !comp.is_empty() && comp != "." && comp != ".." {
            out.push(comp);
        }
    }
    out
}

/// Resolve a Steam Cloud root token (without the surrounding `%`) to an OS directory.
/// `%GameInstall%` needs the game's install directory; the rest derive from the
/// user's home / known folders.
#[cfg(windows)]
fn resolve_root_token(token: &str, install_dir: Option<&Path>) -> Option<PathBuf> {
    let user = || std::env::var_os("USERPROFILE").map(PathBuf::from);
    match token {
        "GameInstall" => install_dir.map(Path::to_path_buf),
        "WinMyDocuments" | "WinDocuments" => user().map(|u| u.join("Documents")),
        "WinAppDataLocal" => std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| user().map(|u| u.join("AppData").join("Local"))),
        "WinAppDataLocalLow" => user().map(|u| u.join("AppData").join("LocalLow")),
        "WinAppDataRoaming" => std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| user().map(|u| u.join("AppData").join("Roaming"))),
        "WinSavedGames" => user().map(|u| u.join("Saved Games")),
        _ => None,
    }
}

/// Linux mapping. `%Win*%` tokens belong to a Proton prefix that this layer doesn't
/// track, so they resolve to `None` (the file is skipped with a warning).
#[cfg(not(windows))]
fn resolve_root_token(token: &str, install_dir: Option<&Path>) -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    match token {
        "GameInstall" => install_dir.map(Path::to_path_buf),
        "LinuxHome" => home(),
        "LinuxXdgDataHome" => std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".local/share"))),
        "LinuxXdgConfigHome" => std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".config"))),
        _ => None,
    }
}

/// What `sync` should do with a single file after comparing the local copy, the
/// cloud copy and the recorded baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlannedAction {
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
struct LocalInfo {
    hash: String,
    size: u64,
    timestamp: u64,
}

impl LocalInfo {
    fn as_baseline(&self) -> BaselineEntry {
        BaselineEntry {
            hash: self.hash.clone(),
            timestamp: self.timestamp,
            size: self.size,
        }
    }
}

/// Read a local file's size, mtime (epoch secs) and content hash. `None` when the
/// path is absent or isn't a regular file (so it counts as "not present locally").
fn read_local_info(path: &Path) -> Option<LocalInfo> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let timestamp = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
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
fn cloud_baseline(cloud: &CloudFileEntry, local_path: &Path) -> BaselineEntry {
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
fn plan_action(
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

/// Stamp a downloaded save with its cloud mtime so the next `sync_up` doesn't see it
/// as locally modified and re-upload it (and so Steam's own comparison stays stable).
fn set_file_mtime_secs(path: &Path, secs: u64) {
    let mtime = filetime::FileTime::from_unix_time(secs as i64, 0);
    if let Err(e) = filetime::set_file_mtime(path, mtime) {
        tracing::warn!("could not set mtime on {}: {e}", path.display());
    }
}

pub struct CloudClient {
    connection: Connection,
    steam_id: u64,
    http: reqwest::Client,
}

impl CloudClient {
    pub fn new(connection: Connection) -> Self {
        let steam_id = u64::from(connection.steam_id());
        Self {
            connection,
            steam_id,
            http: reqwest::Client::new(),
        }
    }

    pub fn steam_id(&self) -> u64 {
        self.steam_id
    }

    pub async fn get_file_list(&self, appid: u32) -> Result<Vec<CloudFileEntry>> {
        let request = CCloud_EnumerateUserFiles_Request {
            appid: Some(appid),
            extended_details: Some(true),
            ..Default::default()
        };

        let response: CCloud_EnumerateUserFiles_Response = self
            .connection
            .service_method(request)
            .await
            .context("failed calling Cloud.EnumerateUserFiles")?;

        Ok(response
            .files
            .into_iter()
            .map(|file| CloudFileEntry {
                filename: file.filename().to_string(),
                timestamp: file.timestamp(),
                size: u64::from(file.file_size()),
                sha_hash: file.file_sha,
            })
            .collect())
    }

    /// Download cloud saves that are newer than (or missing) their local copy.
    /// Thin wrapper over [`sync`](Self::sync) restricted to the download direction;
    /// `Detect` so a divergent file is reported, never silently overwritten.
    pub async fn sync_down(&self, appid: u32, resolver: &CloudPathResolver) -> Result<SyncOutcome> {
        self.sync(appid, resolver, &[], SyncDirection::Down, ConflictPolicy::Detect)
            .await
    }

    /// Upload local saves that are new or newer than their cloud copy. Thin wrapper
    /// over [`sync`](Self::sync) restricted to the upload direction.
    pub async fn sync_up(
        &self,
        appid: u32,
        resolver: &CloudPathResolver,
        specs: &[UfsSaveSpec],
    ) -> Result<SyncOutcome> {
        self.sync(appid, resolver, specs, SyncDirection::Up, ConflictPolicy::Detect)
            .await
    }

    /// Reconcile a game's Steam Cloud saves with their on-disk copies.
    ///
    /// For each save it compares the **content hash** of the cloud and local copies
    /// against a persisted [`SyncBaseline`] (the last state at which the two agreed):
    ///
    /// - identical content → nothing to do;
    /// - only one side changed since the baseline → move it the obvious way
    ///   (upload a newer local save, download a newer cloud save);
    /// - **both** sides changed independently → a *conflict*: the two copies have
    ///   diverged and picking one by timestamp could throw away real progress.
    ///   `policy` decides what happens — [`ConflictPolicy::Detect`] reports it for a
    ///   Take-Cloud/Take-Local choice without touching either copy, while
    ///   `TakeCloud`/`TakeLocal` apply that choice.
    ///
    /// `direction` filters only the *automatic* transfers (`Down`/`Up`/`Both`);
    /// conflicts are always detected, and an explicit `policy` resolution always
    /// runs regardless of direction.
    pub async fn sync(
        &self,
        appid: u32,
        resolver: &CloudPathResolver,
        specs: &[UfsSaveSpec],
        direction: SyncDirection,
        policy: ConflictPolicy,
    ) -> Result<SyncOutcome> {
        let remote_map: HashMap<String, CloudFileEntry> = self
            .get_file_list(appid)
            .await?
            .into_iter()
            .map(|e| (e.filename.clone(), e))
            .collect();

        // Candidate filenames: every cloud file, plus local saves the UFS rules
        // surface that aren't in the cloud yet (so first-time uploads are included).
        let mut names: BTreeSet<String> = remote_map.keys().cloned().collect();
        let mut local_only: HashMap<String, PathBuf> = HashMap::new();
        for (cloud_name, local_path) in discover_local_saves(specs, resolver) {
            if !remote_map.contains_key(&cloud_name) {
                names.insert(cloud_name.clone());
                local_only.insert(cloud_name, local_path);
            }
        }

        let mut baseline = SyncBaseline::load(appid);
        let mut outcome = SyncOutcome::default();

        for name in names {
            let cloud = remote_map.get(&name);
            let local_path = match local_only.get(&name) {
                Some(p) => p.clone(),
                None => match resolver.resolve(&name) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("skipping cloud file '{name}': {e:#}");
                        continue;
                    }
                },
            };
            let local = read_local_info(&local_path);

            match plan_action(baseline.files.get(&name), local.as_ref(), cloud) {
                PlannedAction::Skip => {
                    // In sync (or nothing to do) — record the agreed fingerprint so
                    // a later one-sided edit is recognised as such.
                    if let (Some(l), Some(_)) = (&local, cloud) {
                        baseline.files.insert(name.clone(), l.as_baseline());
                    }
                }
                PlannedAction::Download => {
                    if direction == SyncDirection::Up {
                        continue; // a pending down-change we're not applying now
                    }
                    let Some(c) = cloud else { continue };
                    self.download_to(appid, &name, &local_path, c.timestamp).await?;
                    baseline
                        .files
                        .insert(name.clone(), cloud_baseline(c, &local_path));
                    outcome.downloaded.push(name.clone());
                }
                PlannedAction::Upload => {
                    if direction == SyncDirection::Down {
                        continue;
                    }
                    let Some(l) = &local else { continue };
                    self.upload_from(appid, &name, &local_path, l.timestamp).await?;
                    baseline.files.insert(name.clone(), l.as_baseline());
                    outcome.uploaded.push(name.clone());
                }
                PlannedAction::Conflict => {
                    let (Some(l), Some(c)) = (&local, cloud) else { continue };
                    match policy {
                        ConflictPolicy::Detect => {
                            outcome.conflicts.push(SyncConflict {
                                filename: name.clone(),
                                local_path: local_path.to_string_lossy().into_owned(),
                                local_hash: l.hash.clone(),
                                local_size: l.size,
                                local_timestamp: l.timestamp,
                                cloud_hash: c.sha_hash.clone().unwrap_or_default(),
                                cloud_size: c.size,
                                cloud_timestamp: c.timestamp,
                            });
                        }
                        ConflictPolicy::TakeCloud => {
                            self.download_to(appid, &name, &local_path, c.timestamp).await?;
                            baseline
                                .files
                                .insert(name.clone(), cloud_baseline(c, &local_path));
                            outcome.downloaded.push(name.clone());
                        }
                        ConflictPolicy::TakeLocal => {
                            self.upload_from(appid, &name, &local_path, l.timestamp).await?;
                            baseline.files.insert(name.clone(), l.as_baseline());
                            outcome.uploaded.push(name.clone());
                        }
                    }
                }
            }
        }

        baseline.save(appid);
        Ok(outcome)
    }

    /// Download one cloud file to `local_path` and stamp its mtime to the cloud's,
    /// so the next sync sees the two as in step rather than locally modified.
    async fn download_to(
        &self,
        appid: u32,
        filename: &str,
        local_path: &Path,
        cloud_timestamp: u64,
    ) -> Result<()> {
        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
        }
        let body = self
            .download_file(appid, filename)
            .await
            .with_context(|| format!("failed downloading cloud file '{filename}' for app {appid}"))?;
        tokio::fs::write(local_path, &body)
            .await
            .with_context(|| format!("failed writing {}", local_path.display()))?;
        set_file_mtime_secs(local_path, cloud_timestamp);
        Ok(())
    }

    async fn upload_from(
        &self,
        appid: u32,
        filename: &str,
        local_path: &Path,
        local_timestamp: u64,
    ) -> Result<()> {
        let data = tokio::fs::read(local_path)
            .await
            .with_context(|| format!("failed reading {}", local_path.display()))?;
        self.upload_file(appid, filename, local_timestamp, data)
            .await
            .with_context(|| format!("failed uploading cloud file '{filename}' for app {appid}"))
    }

    async fn download_file(&self, appid: u32, filename: &str) -> Result<Vec<u8>> {
        let request = CCloud_ClientFileDownload_Request {
            appid: Some(appid),
            filename: Some(filename.to_string()),
            ..Default::default()
        };

        let response: CCloud_ClientFileDownload_Response = self
            .connection
            .service_method(request)
            .await
            .context("failed calling Cloud.ClientFileDownload")?;

        let url = cloud_transfer_url(
            response.use_https(),
            response.url_host(),
            response.url_path(),
            "ClientFileDownload",
        )?;
        let headers = build_header_map(response.request_headers.iter().map(|h| {
            (
                h.name.as_deref().unwrap_or_default(),
                h.value.as_deref().unwrap_or_default(),
            )
        }))?;

        let response = self
            .http
            .get(url)
            .headers(headers)
            .send()
            .await
            .context("failed cloud HTTP GET")?
            .error_for_status()
            .context("cloud HTTP GET returned failure status")?;

        Ok(response
            .bytes()
            .await
            .context("failed reading cloud download body")?
            .to_vec())
    }

    async fn upload_file(
        &self,
        appid: u32,
        filename: &str,
        timestamp: u64,
        data: Vec<u8>,
    ) -> Result<()> {
        let mut sha = Sha1::new();
        sha.update(&data);
        let file_sha = sha.finalize().to_vec();
        let file_size = u32::try_from(data.len()).context("cloud upload larger than u32")?;

        let begin_request = CCloud_ClientBeginFileUpload_Request {
            appid: Some(appid),
            file_size: Some(file_size),
            raw_file_size: Some(file_size),
            file_sha: Some(file_sha.clone()),
            time_stamp: Some(timestamp),
            filename: Some(filename.to_string()),
            ..Default::default()
        };

        let begin_response: CCloud_ClientBeginFileUpload_Response = self
            .connection
            .service_method(begin_request)
            .await
            .context("failed calling Cloud.ClientBeginFileUpload")?;

        for mut block in begin_response.block_requests {
            let url = cloud_transfer_url(
                block.use_https(),
                block.url_host(),
                block.url_path(),
                "ClientBeginFileUpload",
            )?;

            let block_offset = usize::try_from(block.block_offset()).unwrap_or(0);
            let block_length = usize::try_from(block.block_length()).unwrap_or(data.len());
            let end = block_offset.saturating_add(block_length).min(data.len());
            let payload = if block.explicit_body_data.is_some() {
                block.take_explicit_body_data()
            } else if block_offset < data.len() {
                data[block_offset..end].to_vec()
            } else {
                data.clone()
            };

            let method = match block.http_method() {
                1 => reqwest::Method::PUT,
                2 => reqwest::Method::POST,
                _ => reqwest::Method::PUT,
            };
            let headers = build_header_map(block.request_headers.iter().map(|h| {
                (
                    h.name.as_deref().unwrap_or_default(),
                    h.value.as_deref().unwrap_or_default(),
                )
            }))?;

            self.http
                .request(method, url)
                .headers(headers)
                .body(payload)
                .send()
                .await
                .context("failed cloud HTTP upload")?
                .error_for_status()
                .context("cloud HTTP upload returned failure status")?;
        }

        let commit_request = CCloud_ClientCommitFileUpload_Request {
            transfer_succeeded: Some(true),
            appid: Some(appid),
            file_sha: Some(file_sha),
            filename: Some(filename.to_string()),
            ..Default::default()
        };

        let commit_response: CCloud_ClientCommitFileUpload_Response = self
            .connection
            .service_method(commit_request)
            .await
            .context("failed calling Cloud.ClientCommitFileUpload")?;

        if !commit_response.file_committed() {
            return Err(anyhow!(
                "Cloud.ClientCommitFileUpload returned file_committed=false"
            ));
        }

        Ok(())
    }
}

/// Build the request URL for a Steam Cloud HTTP transfer from the scheme flag and
/// host/path the service returned, rejecting the empty host/path Steam sends when a
/// transfer slot is unavailable. `what` names the originating RPC for the error.
fn cloud_transfer_url(use_https: bool, host: &str, path: &str, what: &str) -> Result<String> {
    if host.is_empty() || path.is_empty() {
        return Err(anyhow!("{what} returned empty URL host/path"));
    }
    let scheme = if use_https { "https" } else { "http" };
    Ok(format!("{scheme}://{host}{path}"))
}

fn build_header_map<'a>(headers: impl Iterator<Item = (&'a str, &'a str)>) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        if name.is_empty() {
            continue;
        }

        let header_name = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid header name '{name}'"))?;
        let header_value = HeaderValue::from_str(value)
            .with_context(|| format!("invalid header value for '{name}'"))?;
        map.insert(header_name, header_value);
    }
    Ok(map)
}

pub fn default_cloud_root(steam_id: u64, appid: u32) -> Result<PathBuf> {
    let home = crate::core::config::home_dir()?;
    let account_id = steam_id as u32;
    Ok(home
        .join(".local/share/Steam/userdata")
        .join(account_id.to_string())
        .join(appid.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_root_token() {
        assert_eq!(
            split_root_token("%WinAppDataLocalLow%SadSocket/9Kings/save.json"),
            Some(("WinAppDataLocalLow", "SadSocket/9Kings/save.json")),
        );
        // Classic remote-storage names have no token.
        assert_eq!(split_root_token("savegame.dat"), None);
    }

    #[test]
    fn join_relative_is_bounded() {
        let base = Path::new("/base");
        // Leading slash, `.` and `..` components are ignored — no escaping the base.
        assert_eq!(
            join_relative(base, "/a/./b/../c"),
            Path::new("/base").join("a").join("b").join("c"),
        );
    }

    #[test]
    fn classic_file_resolves_under_remote_root() {
        let resolver = CloudPathResolver::new(PathBuf::from("/r/remote"), None);
        assert_eq!(
            resolver.resolve("sub/save.dat").unwrap(),
            Path::new("/r/remote").join("sub").join("save.dat"),
        );
    }

    #[cfg(windows)]
    #[test]
    fn auto_cloud_token_maps_to_real_os_path() {
        // The exact case that was failing: the file landed in a phantom
        // `%WinAppDataLocalLow%SadSocket` folder instead of real LocalLow.
        let resolver = CloudPathResolver::new(PathBuf::from(r"C:\r\remote"), None);
        let resolved = resolver
            .resolve("%WinAppDataLocalLow%SadSocket/9Kings/9KingsSettings.json")
            .unwrap();
        let user = PathBuf::from(std::env::var_os("USERPROFILE").unwrap());
        assert_eq!(
            resolved,
            user.join("AppData")
                .join("LocalLow")
                .join("SadSocket")
                .join("9Kings")
                .join("9KingsSettings.json"),
        );
    }

    fn cloud(hash: &str, ts: u64) -> CloudFileEntry {
        CloudFileEntry {
            filename: "save.dat".to_string(),
            timestamp: ts,
            size: 10,
            sha_hash: Some(hash.to_string()),
        }
    }
    fn local(hash: &str, ts: u64) -> LocalInfo {
        LocalInfo { hash: hash.to_string(), size: 10, timestamp: ts }
    }
    fn base(hash: &str) -> BaselineEntry {
        BaselineEntry { hash: hash.to_string(), timestamp: 0, size: 10 }
    }

    #[test]
    fn identical_content_is_skipped_regardless_of_timestamp() {
        // Same bytes on both sides but different mtimes must NOT cause a transfer.
        let action = plan_action(None, Some(&local("aaaa", 200)), Some(&cloud("AAAA", 100)));
        assert_eq!(action, PlannedAction::Skip, "hash match wins over timestamps");
    }

    #[test]
    fn only_local_changed_uploads_not_conflicts() {
        // The normal play loop: baseline==cloud, the user played so local advanced.
        // This must be a plain upload, never a conflict prompt.
        let action = plan_action(Some(&base("cccc")), Some(&local("llll", 300)), Some(&cloud("cccc", 100)));
        assert_eq!(action, PlannedAction::Upload);
    }

    #[test]
    fn only_cloud_changed_downloads() {
        // Played on another machine: baseline==local, cloud advanced. Plain download.
        let action = plan_action(Some(&base("llll")), Some(&local("llll", 100)), Some(&cloud("cccc", 300)));
        assert_eq!(action, PlannedAction::Download);
    }

    #[test]
    fn both_changed_is_a_conflict() {
        // Both sides diverged from the last-synced baseline — the data-loss case.
        let action = plan_action(Some(&base("oldd")), Some(&local("llll", 300)), Some(&cloud("cccc", 290)));
        assert_eq!(action, PlannedAction::Conflict);
    }

    #[test]
    fn first_sync_with_two_differing_copies_is_a_conflict() {
        // No baseline yet and the copies differ: we can't know which lineage wins.
        let action = plan_action(None, Some(&local("llll", 300)), Some(&cloud("cccc", 100)));
        assert_eq!(action, PlannedAction::Conflict);
    }

    #[test]
    fn one_sided_presence_moves_the_only_way_it_can() {
        assert_eq!(plan_action(None, Some(&local("llll", 1)), None), PlannedAction::Upload);
        assert_eq!(plan_action(None, None, Some(&cloud("cccc", 1))), PlannedAction::Download);
    }

    #[test]
    fn glob_matches_handles_wildcards_case_insensitively() {
        assert!(glob_matches("*", "anything.dat"));
        assert!(glob_matches("", "anything.dat"));
        assert!(glob_matches("*.sav", "Game01.SAV"));
        assert!(glob_matches("save?.dat", "save7.dat"));
        assert!(!glob_matches("*.sav", "notes.txt"));
    }

    #[test]
    fn discovers_local_saves_from_ufs_specs() {
        // %GameInstall% lets us point a UFS rule at a temp dir without touching real
        // user folders. A new save under it must be discovered and named correctly.
        let tmp = tempfile::tempdir().unwrap();
        let saves = tmp.path().join("saves").join("slot1");
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::write(saves.join("game.sav"), b"data").unwrap();
        std::fs::write(saves.join("ignore.txt"), b"x").unwrap();

        let resolver =
            CloudPathResolver::new(PathBuf::from("/unused"), Some(tmp.path().to_path_buf()));
        let specs = [UfsSaveSpec {
            root: "GameInstall".to_string(),
            path: "saves".to_string(),
            pattern: "*.sav".to_string(),
            recursive: true,
        }];

        let found = discover_local_saves(&specs, &resolver);
        assert_eq!(found.len(), 1, "only the .sav should match");
        assert_eq!(found[0].0, "%GameInstall%saves/slot1/game.sav");
        assert_eq!(found[0].1, saves.join("game.sav"));
        // And the produced cloud name round-trips back to the same local path.
        assert_eq!(resolver.resolve(&found[0].0).unwrap(), saves.join("game.sav"));
    }

    #[test]
    fn non_recursive_spec_skips_subdirectories() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("top.sav"), b"a").unwrap();
        let sub = tmp.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("deep.sav"), b"b").unwrap();

        let resolver =
            CloudPathResolver::new(PathBuf::from("/unused"), Some(tmp.path().to_path_buf()));
        let specs = [UfsSaveSpec {
            root: "GameInstall".to_string(),
            path: String::new(),
            pattern: "*".to_string(),
            recursive: false,
        }];

        let names: Vec<_> = discover_local_saves(&specs, &resolver)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(names, vec!["%GameInstall%top.sav".to_string()]);
    }

    #[cfg(windows)]
    #[test]
    fn game_install_token_needs_install_dir() {
        let with_dir =
            CloudPathResolver::new(PathBuf::from(r"C:\r"), Some(PathBuf::from(r"C:\games\foo")));
        assert_eq!(
            with_dir.resolve("%GameInstall%saves/a.sav").unwrap(),
            Path::new(r"C:\games\foo").join("saves").join("a.sav"),
        );
        // Without an install dir the token can't be resolved.
        let without = CloudPathResolver::new(PathBuf::from(r"C:\r"), None);
        assert!(without.resolve("%GameInstall%saves/a.sav").is_err());
    }
}
