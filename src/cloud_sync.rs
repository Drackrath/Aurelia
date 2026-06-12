use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use sha1::{Digest, Sha1};
use std::collections::{BTreeMap, HashMap};
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
                sha_hash: file.file_sha.clone(),
            })
            .collect())
    }

    /// Download cloud saves that are newer than (or missing) their local copy,
    /// writing each to its real OS path as mapped by `resolver`.
    pub async fn sync_down(&self, appid: u32, resolver: &CloudPathResolver) -> Result<()> {
        let remote_files = self.get_file_list(appid).await?;
        for remote in remote_files {
            let local_path = match resolver.resolve(&remote.filename) {
                Ok(path) => path,
                Err(e) => {
                    tracing::warn!("skipping cloud file '{}': {e:#}", remote.filename);
                    continue;
                }
            };

            let needs_download = match file_modified_epoch_secs(&local_path).await.ok() {
                None => true,
                Some(ts) => remote.timestamp > ts,
            };
            if !needs_download {
                continue;
            }

            if let Some(parent) = local_path.parent() {
                tokio::fs::create_dir_all(parent).await.with_context(|| {
                    format!("failed to create parent directory {}", parent.display())
                })?;
            }

            let body = self
                .download_file(appid, &remote.filename)
                .await
                .with_context(|| {
                    format!(
                        "failed downloading cloud file '{}' for app {}",
                        remote.filename, appid
                    )
                })?;

            tokio::fs::write(&local_path, &body)
                .await
                .with_context(|| format!("failed writing {}", local_path.display()))?;
            set_file_mtime_secs(&local_path, remote.timestamp);
        }

        Ok(())
    }

    /// Upload local saves that are new, newer, or differ in size from their cloud
    /// copy. The candidate set is the union of (a) files already in the cloud, mapped
    /// back to their real OS path, and (b) local files matched by the app's UFS
    /// `savefiles` rules (`specs`) — so a brand-new save that has never been in the
    /// cloud still gets its first upload. Pass an empty `specs` to update existing
    /// cloud files only.
    pub async fn sync_up(
        &self,
        appid: u32,
        resolver: &CloudPathResolver,
        specs: &[UfsSaveSpec],
    ) -> Result<()> {
        let remote_map: HashMap<String, CloudFileEntry> = self
            .get_file_list(appid)
            .await?
            .into_iter()
            .map(|e| (e.filename.clone(), e))
            .collect();

        // Candidate cloud-name -> local-path. Cloud files first, then discovered
        // local saves (which only fill in names the cloud doesn't already have).
        let mut candidates: BTreeMap<String, PathBuf> = BTreeMap::new();
        for name in remote_map.keys() {
            match resolver.resolve(name) {
                Ok(path) => {
                    candidates.insert(name.clone(), path);
                }
                Err(e) => tracing::warn!("skipping cloud file '{name}': {e:#}"),
            }
        }
        for (cloud_name, local_path) in discover_local_saves(specs, resolver) {
            candidates.entry(cloud_name).or_insert(local_path);
        }

        for (cloud_name, local_path) in candidates {
            let metadata = match tokio::fs::metadata(&local_path).await {
                Ok(m) => m,
                Err(_) => continue, // not present locally — nothing to upload
            };
            let local_timestamp = metadata
                .modified()
                .ok()
                .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or_default();
            let local_size = metadata.len();

            let should_upload = match remote_map.get(&cloud_name) {
                None => true, // new file, not yet in the cloud
                Some(remote) => local_timestamp > remote.timestamp || local_size != remote.size,
            };
            if !should_upload {
                continue;
            }

            let data = tokio::fs::read(&local_path)
                .await
                .with_context(|| format!("failed reading {}", local_path.display()))?;
            self.upload_file(appid, &cloud_name, local_timestamp, data)
                .await
                .with_context(|| {
                    format!("failed uploading cloud file '{cloud_name}' for app {appid}")
                })?;
        }

        Ok(())
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

        let scheme = if response.use_https() {
            "https"
        } else {
            "http"
        };
        let host = response.url_host();
        let path = response.url_path();
        if host.is_empty() || path.is_empty() {
            return Err(anyhow!("ClientFileDownload returned empty URL host/path"));
        }

        let url = format!("{scheme}://{host}{path}");
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

        let begin_request = CCloud_ClientBeginFileUpload_Request {
            appid: Some(appid),
            file_size: Some(u32::try_from(data.len()).context("cloud upload larger than u32")?),
            raw_file_size: Some(u32::try_from(data.len()).context("cloud upload larger than u32")?),
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
            let scheme = if block.use_https() { "https" } else { "http" };
            let host = block.url_host().to_string();
            let path = block.url_path().to_string();
            if host.is_empty() || path.is_empty() {
                return Err(anyhow!(
                    "ClientBeginFileUpload returned empty URL host/path"
                ));
            }

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

            let url = format!("{scheme}://{host}{path}");
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

async fn file_modified_epoch_secs(path: &Path) -> Result<u64> {
    let metadata = tokio::fs::metadata(path).await?;
    let modified = metadata.modified()?;
    let seconds = modified
        .duration_since(UNIX_EPOCH)
        .context("invalid file modified timestamp")?
        .as_secs();
    Ok(seconds)
}

pub fn default_cloud_root(steam_id: u64, appid: u32) -> Result<PathBuf> {
    let home = crate::config::home_dir()?;
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
