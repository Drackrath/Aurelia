use crate::models::{OwnedGame, SessionState, SteamPrefixMode, UserConfigStore};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;

/// Serialize `value` as pretty JSON and write it to `path`, creating the parent
/// directory first. Shared by the `save_*` helpers so the create-dir / serialize /
/// write / error-context sequence lives in one place.
async fn write_json_pretty<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed creating {}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(value)?;
    fs::write(path, body)
        .await
        .with_context(|| format!("failed writing {}", path.display()))?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct GameConfig {
    pub forced_proton_version: Option<String>,
    pub platform_preference: Option<String>,
}

/// Steam presence the session daemon
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChatPresence {
    #[default]
    Offline,
    Online,
}

impl ChatPresence {
    /// The raw EPersonaState
    pub fn persona_state(self) -> u32 {
        match self {
            ChatPresence::Offline => 7,
            ChatPresence::Online => 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LauncherConfig {
    pub steam_library_path: String,
    pub proton_version: String,
    // (default: offline).
    #[serde(default)]
    pub chat_presence: ChatPresence,
    #[serde(default)]
    pub steam_runtime_runner: PathBuf,
    #[serde(default)]
    pub steam_prefix_mode: SteamPrefixMode,
    pub enable_cloud_sync: bool,
    #[serde(default)]
    pub use_shared_compat_data: bool,
    #[serde(default = "crate::models::default_true")]
    pub windows_steam_discovery_enabled: bool,
    #[serde(default)]
    pub preferred_launch_options: HashMap<u32, String>,
    #[serde(default)]
    pub game_configs: HashMap<u32, GameConfig>,
}

impl LauncherConfig {
    pub async fn load() -> Result<Self> {
        load_launcher_config().await
    }

    pub async fn save(&self) -> Result<()> {
        save_launcher_config(self).await
    }
}

impl Default for LauncherConfig {
    fn default() -> Self {
        let steam_library_path = detect_steam_path()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                home_dir()
                    .map(|home| home.join("Games").join("Aurelia"))
                    .unwrap_or_else(|_| PathBuf::from("~/Games/Aurelia"))
                    .to_string_lossy()
                    .into_owned()
            });

        Self {
            steam_library_path,
            proton_version: "experimental".to_string(),
            chat_presence: ChatPresence::default(),
            steam_runtime_runner: PathBuf::new(),
            steam_prefix_mode: SteamPrefixMode::default(),
            enable_cloud_sync: true,
            use_shared_compat_data: false,
            windows_steam_discovery_enabled: true,
            preferred_launch_options: HashMap::new(),
            game_configs: HashMap::new(),
        }
    }
}

pub fn detect_steam_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let candidates = [PathBuf::from(r"C:\Program Files (x86)\Steam")];
        return candidates.into_iter().find(|path| path.exists());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let home = std::env::var("HOME").ok()?;
        let candidates = [
            PathBuf::from(&home).join(".steam/steam"),
            PathBuf::from(&home).join(".local/share/Steam"),
            PathBuf::from(&home).join(".steam/root"),
        ];
        candidates.into_iter().find(|path| path.exists())
    }
}

/// Resolve the user's home directory in a cross-platform way.
///
/// On Unix this is `HOME`; on Windows that variable is normally unset, so we
/// fall back to `USERPROFILE` (and finally `HOMEDRIVE`+`HOMEPATH`).
pub fn home_dir() -> Result<PathBuf> {
    let from = |var: &str| std::env::var_os(var).filter(|v| !v.is_empty());

    if let Some(home) = from("HOME").or_else(|| from("USERPROFILE")) {
        return Ok(PathBuf::from(home));
    }

    if let (Some(drive), Some(path)) = (from("HOMEDRIVE"), from("HOMEPATH")) {
        let mut combined = PathBuf::from(drive);
        combined.push(path);
        return Ok(combined);
    }

    anyhow::bail!("could not determine home directory (none of HOME, USERPROFILE, or HOMEDRIVE/HOMEPATH are set)")
}

pub fn config_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".config/Aurelia"))
}

pub async fn ensure_config_dirs() -> Result<()> {
    let config = config_dir()?;
    fs::create_dir_all(&config).await?;
    let images = opensteam_image_cache_dir()?;
    fs::create_dir_all(&images).await?;
    Ok(())
}

pub fn opensteam_image_cache_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("images"))
}

pub fn data_dir() -> Result<PathBuf> {
    config_dir()
}

pub async fn load_session() -> Result<SessionState> {
    let session_path = config_dir()?.join("session.json");
    if !session_path.exists() {
        return Ok(SessionState::default());
    }

    let raw = fs::read_to_string(&session_path)
        .await
        .with_context(|| format!("failed reading {}", session_path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed parsing {}", session_path.display()))
}

pub async fn save_session(session: &SessionState) -> Result<()> {
    let session_path = config_dir()?.join("session.json");
    write_json_pretty(&session_path, session).await?;

    // The session holds a long-lived Steam refresh token; keep it owner-only so it
    // is not world-readable on shared Unix hosts.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&session_path, std::fs::Permissions::from_mode(0o600))
            .await
            .with_context(|| format!("failed securing {}", session_path.display()))?;
    }

    Ok(())
}

pub async fn delete_session() -> Result<()> {
    let session_path = config_dir()?.join("session.json");
    if session_path.exists() {
        fs::remove_file(session_path).await?;
    }
    Ok(())
}

pub async fn load_launcher_config() -> Result<LauncherConfig> {
    let path = config_dir()?.join("config.json");
    if !path.exists() {
        let mut config = LauncherConfig::default();
        if let Some(detected) = detect_steam_path() {
            config.steam_library_path = detected.to_string_lossy().to_string();
        }
        return Ok(config);
    }

    let raw = fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed reading {}", path.display()))?;
    serde_json::from_str::<LauncherConfig>(&raw)
        .with_context(|| format!("failed parsing {}", path.display()))
}

pub async fn save_launcher_config(config: &LauncherConfig) -> Result<()> {
    let path = config_dir()?.join("config.json");
    write_json_pretty(&path, config).await
}

pub fn library_cache_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("library_cache.json"))
}

pub async fn save_library_cache(owned_games: &[OwnedGame]) -> Result<()> {
    write_json_pretty(&library_cache_path()?, &owned_games).await
}

pub async fn load_library_cache() -> Result<Vec<OwnedGame>> {
    let path = library_cache_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw = fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed reading {}", path.display()))?;
    serde_json::from_str::<Vec<OwnedGame>>(&raw)
        .with_context(|| format!("failed parsing {}", path.display()))
}

/// Default lifetime of a cached `aurelia info` record before it is re-fetched
/// from Steam. Store metadata is effectively static for hours, while drivers like
/// Heroic call `info` repeatedly; caching it collapses those into one Steam CM
/// logon + StoreBrowse/PICS round-trip per app per window. Override (in seconds)
/// with `AURELIA_INFO_CACHE_TTL` — `0` disables the cache.
const INFO_CACHE_DEFAULT_TTL_SECS: u64 = 6 * 60 * 60;

/// The configured `info` cache TTL ([`INFO_CACHE_DEFAULT_TTL_SECS`] unless
/// `AURELIA_INFO_CACHE_TTL` overrides it).
pub fn info_cache_ttl() -> std::time::Duration {
    let secs = std::env::var("AURELIA_INFO_CACHE_TTL")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(INFO_CACHE_DEFAULT_TTL_SECS);
    std::time::Duration::from_secs(secs)
}

/// A cached `aurelia info` result: the StoreBrowse metadata plus resolved DLC
/// `(app_id, name)` pairs, stamped with the unix time it was fetched so a reader
/// can honour a TTL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedAppInfo {
    pub fetched_at: u64,
    pub details: crate::steam_client::StoreAppInfo,
    pub dlc: Vec<(u32, Option<String>)>,
}

fn info_cache_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("info_cache"))
}

/// Per-app cache file. One file per app id (rather than a shared map) so
/// concurrent Aurelia invocations for different apps never clobber each other.
fn info_cache_path(app_id: u32) -> Result<PathBuf> {
    Ok(info_cache_dir()?.join(format!("{app_id}.json")))
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Load a cached `info` record for `app_id` if one exists and is still within
/// `ttl`. Returns `None` on a miss, a stale entry, or any read/parse error, so the
/// caller falls through to a fresh fetch. A zero `ttl` always misses.
pub async fn load_info_cache(app_id: u32, ttl: std::time::Duration) -> Option<CachedAppInfo> {
    if ttl.is_zero() {
        return None;
    }
    let path = info_cache_path(app_id).ok()?;
    let raw = fs::read_to_string(&path).await.ok()?;
    let cached: CachedAppInfo = serde_json::from_str(&raw).ok()?;
    let age = now_unix().saturating_sub(cached.fetched_at);
    (age <= ttl.as_secs()).then_some(cached)
}

/// Persist an `info` record for `app_id`, stamped with the current time.
pub async fn save_info_cache(
    app_id: u32,
    details: &crate::steam_client::StoreAppInfo,
    dlc: &[(u32, Option<String>)],
) -> Result<()> {
    let record = CachedAppInfo {
        fetched_at: now_unix(),
        details: details.clone(),
        dlc: dlc.to_vec(),
    };
    write_json_pretty(&info_cache_path(app_id)?, &record).await
}

pub async fn load_user_configs() -> Result<UserConfigStore> {
    let path = config_dir()?.join("user_apps.json");
    if !path.exists() {
        return Ok(UserConfigStore::new());
    }

    let raw = fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed reading {}", path.display()))?;
    serde_json::from_str::<UserConfigStore>(&raw)
        .with_context(|| format!("failed parsing {}", path.display()))
}

pub async fn save_user_configs(configs: &UserConfigStore) -> Result<()> {
    let path = config_dir()?.join("user_apps.json");
    write_json_pretty(&path, configs).await
}
