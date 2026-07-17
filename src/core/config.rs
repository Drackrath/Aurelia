use crate::core::models::{OwnedGame, SessionState, SteamPrefixMode, UserConfigStore};
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
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

/// Read `path` and parse it as JSON into `T`. Shared by the `load_*` helpers so the
/// read / parse / error-context sequence lives in one place. Callers handle the
/// missing-file fallback themselves, since each returns a different default.
async fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let raw = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed reading {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed parsing {}", path.display()))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct GameConfig {
    pub forced_proton_version: Option<String>,
    pub platform_preference: Option<String>,
    /// Which launch backend this game uses. `Auto` (default) keeps Aurelia's normal
    /// native-vs-Proton selection; `Luxtorpeda` routes the game through the optional
    /// luxtorpeda native-engine plugin (Linux only, requires `luxtorpeda_enabled`).
    #[serde(default)]
    pub runner: GameRunner,
    /// Per-game launch script path. When set (and not overridden/bypassed at launch),
    /// Aurelia wraps the resolved launch command with this script. Takes precedence
    /// over the auto-detected `<script_dir>/<app_id>.sh|bat`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_script: Option<String>,
    /// Depot → manifest ids this game is pinned to (recorded when the game is
    /// downgraded or pinned). Used to display what an update lock is holding the
    /// game at; empty when the game isn't pinned.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub pinned_manifests: HashMap<u32, u64>,
    /// Whether Aurelia's update commands are locked for this game. A pinned game is
    /// not upgraded by `update` / `check-updates` (which report it as pinned).
    #[serde(default)]
    pub pinned: bool,
}

/// Per-game launch backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GameRunner {
    /// Aurelia's normal selection (native Linux executable, or Proton/Wine for Windows).
    #[default]
    Auto,
    /// Route through the luxtorpeda native-engine plugin.
    Luxtorpeda,
    /// Route through the umu-launcher plugin (Proton via umu; Linux only, requires `umu_enabled`).
    Umu,
}

/// Network proxy settings applied to all of Aurelia's HTTP(S) traffic: the Steam
/// Community/store/market web endpoints, depot content downloads (steam-cdn), and the
/// GitHub/Codeberg release lookups used by the Proton/plugin managers. Empty by default
/// (a direct connection). See [`crate::core::net`] for how these are applied.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ProxyConfig {
    /// Proxy URL for all HTTP(S) requests, e.g. `http://host:8080`,
    /// `http://user:pass@host:8080`, or `socks5://host:1080`. `None` = direct.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Comma-separated hosts/domains that bypass the proxy (`NO_PROXY` semantics),
    /// e.g. `localhost,127.0.0.1,.internal`. `None` = no exceptions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_proxy: Option<String>,
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
    #[serde(default = "crate::core::models::default_true")]
    pub windows_steam_discovery_enabled: bool,
    #[serde(default)]
    pub preferred_launch_options: HashMap<u32, String>,
    #[serde(default)]
    pub game_configs: HashMap<u32, GameConfig>,
    /// Master toggle for the optional luxtorpeda native-engine plugin. When `false`
    /// (default) nothing is ever downloaded and no game is routed through luxtorpeda.
    #[serde(default)]
    pub luxtorpeda_enabled: bool,
    /// Path to an externally-managed luxtorpeda install (a directory containing, or holding
    /// a subdirectory with, `toolmanifest.vdf`). When set, Aurelia uses this install and
    /// never downloads its own managed copy. `None` (default) uses the on-the-fly download.
    #[serde(default)]
    pub luxtorpeda_path: Option<String>,
    /// Master toggle for the optional umu-launcher plugin. When `false` (default)
    /// nothing is ever downloaded and no game is routed through umu.
    #[serde(default)]
    pub umu_enabled: bool,
    /// Path to an externally-managed umu install (a directory containing `umu-run`, or the
    /// `umu-run` binary itself). When set, Aurelia uses this install and never downloads its
    /// own managed copy. `None` (default) uses the on-the-fly download.
    #[serde(default)]
    pub umu_path: Option<String>,
    /// Persistent default Steam API language *name* (e.g. "german") used by
    /// `aurelia achievements` when `--lang` is not given. `None` = use "english".
    #[serde(default)]
    pub language: Option<String>,
    /// Network proxy for all HTTP(S) communication. Empty by default (a direct
    /// connection). Applied process-wide at startup; see [`crate::core::net`].
    #[serde(default)]
    pub proxy: ProxyConfig,
    /// Opt-in gate for experimental features. Off by default. Currently gates the
    /// browser/OpenID identity check (`login --openid`) and web-token auth
    /// (`login --web-token`), which prove identity / enable web-surface commands
    /// but cannot create the full client session that library/install/launch need.
    /// Enable with `aurelia config experimental true` or `AURELIA_EXPERIMENTAL=1`.
    #[serde(default)]
    pub experimental: bool,
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
            luxtorpeda_enabled: false,
            luxtorpeda_path: None,
            umu_enabled: false,
            umu_path: None,
            language: None,
            proxy: ProxyConfig::default(),
            experimental: false,
        }
    }
}

/// Whether experimental features are enabled: the `experimental` config toggle,
/// or the `AURELIA_EXPERIMENTAL` environment variable set to a truthy value
/// (anything other than empty, `0`, or `false`). The env var lets a driver or a
/// one-off invocation opt in without persisting the setting.
pub async fn experimental_enabled() -> bool {
    if let Some(value) = std::env::var_os("AURELIA_EXPERIMENTAL") {
        let value = value.to_string_lossy();
        let value = value.trim();
        if !value.is_empty() && !value.eq_ignore_ascii_case("0") && !value.eq_ignore_ascii_case("false")
        {
            return true;
        }
    }
    load_launcher_config()
        .await
        .map(|config| config.experimental)
        .unwrap_or(false)
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
    // embedding driver
    if let Some(dir) = std::env::var_os("AURELIA_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
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
    read_json(&session_path).await
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

/// Load `config.json`.
///
/// A *missing* config is not an error: it yields defaults with the Steam library
/// auto-detected. So an `Err` here always means the file exists but could not be read
/// or parsed — never "you have no config yet". Callers must therefore propagate it
/// rather than falling back to defaults: doing so would silently discard every setting
/// the user has, and any caller that then saves would write those defaults straight
/// over the file it failed to read.
pub async fn load_launcher_config() -> Result<LauncherConfig> {
    let path = config_dir()?.join("config.json");
    if !path.exists() {
        let mut config = LauncherConfig::default();
        if let Some(detected) = detect_steam_path() {
            config.steam_library_path = detected.to_string_lossy().to_string();
        }
        return Ok(config);
    }
    read_json(&path).await.with_context(|| {
        format!(
            "invalid launcher config — fix the JSON in {}, or delete it to regenerate defaults",
            path.display()
        )
    })
}

pub async fn save_launcher_config(config: &LauncherConfig) -> Result<()> {
    let path = config_dir()?.join("config.json");
    write_json_pretty(&path, config).await
}

/// Synchronously read just the proxy settings from `config.json`. Used at process
/// startup — before the async runtime and worker threads exist — to install the proxy
/// environment variables (see [`crate::core::net::install_proxy_env`]), which is why it
/// can't use the async loader. Returns the default (direct connection) on a missing,
/// unreadable, or unparseable config.
pub fn load_proxy_config_blocking() -> ProxyConfig {
    let Ok(path) = config_dir().map(|dir| dir.join("config.json")) else {
        return ProxyConfig::default();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return ProxyConfig::default();
    };
    serde_json::from_str::<LauncherConfig>(&raw)
        .map(|config| config.proxy)
        .unwrap_or_default()
}

/// Pin a game to `manifests` (depot → manifest ids), locking Aurelia's update
/// commands for it. Preserves the game's other `GameConfig` fields.
pub async fn set_game_pin(app_id: u32, manifests: HashMap<u32, u64>) -> Result<()> {
    let mut cfg = load_launcher_config().await?;
    let gc = cfg.game_configs.entry(app_id).or_default();
    gc.pinned = true;
    gc.pinned_manifests = manifests;
    save_launcher_config(&cfg).await
}

/// Clear a game's version pin (unlock Aurelia's update commands). No-op if the
/// game was never pinned.
pub async fn clear_game_pin(app_id: u32) -> Result<()> {
    let mut cfg = load_launcher_config().await?;
    if let Some(gc) = cfg.game_configs.get_mut(&app_id) {
        gc.pinned = false;
        gc.pinned_manifests.clear();
    }
    save_launcher_config(&cfg).await
}

/// Return `(pinned, pinned_manifests)` for a game, reading the launcher config.
/// `(false, empty)` when the game has no pin (or the config can't be read).
pub async fn game_pin_state(app_id: u32) -> (bool, HashMap<u32, u64>) {
    match load_launcher_config().await {
        Ok(cfg) => cfg
            .game_configs
            .get(&app_id)
            .map(|gc| (gc.pinned, gc.pinned_manifests.clone()))
            .unwrap_or((false, HashMap::new())),
        Err(_) => (false, HashMap::new()),
    }
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
    read_json(&path).await
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

/// Per-app, per-language cache file. One file per `(app id, language)` (rather
/// than a shared map) so concurrent Aurelia invocations for different apps — or
/// the same app in different languages — never clobber each other. The language
/// is sanitized to keep it safe as a filename component.
fn info_cache_path(app_id: u32, language: &str) -> Result<PathBuf> {
    let lang: String = language
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    Ok(info_cache_dir()?.join(format!("{app_id}.{lang}.json")))
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
pub async fn load_info_cache(
    app_id: u32,
    language: &str,
    ttl: std::time::Duration,
) -> Option<CachedAppInfo> {
    if ttl.is_zero() {
        return None;
    }
    let path = info_cache_path(app_id, language).ok()?;
    let raw = fs::read_to_string(&path).await.ok()?;
    let cached: CachedAppInfo = serde_json::from_str(&raw).ok()?;
    let age = now_unix().saturating_sub(cached.fetched_at);
    (age <= ttl.as_secs()).then_some(cached)
}

/// Persist an `info` record for `app_id`, stamped with the current time.
pub async fn save_info_cache(
    app_id: u32,
    language: &str,
    details: &crate::steam_client::StoreAppInfo,
    dlc: &[(u32, Option<String>)],
) -> Result<()> {
    let record = CachedAppInfo {
        fetched_at: now_unix(),
        details: details.clone(),
        dlc: dlc.to_vec(),
    };
    write_json_pretty(&info_cache_path(app_id, language)?, &record).await
}

/// Load the per-game user config store (`user_apps.json`).
///
/// A *missing* file is not an error — it yields an empty store. So an `Err` here always
/// means the file exists but could not be parsed, and callers must propagate it rather
/// than falling back to an empty store: silently dropping every per-game setting (launch
/// options, env vars, the Steam-runtime policy) on one typo is exactly the footgun this
/// avoids. See the same contract on [`load_launcher_config`].
pub async fn load_user_configs() -> Result<UserConfigStore> {
    let path = config_dir()?.join("user_apps.json");
    if !path.exists() {
        return Ok(UserConfigStore::new());
    }
    read_json(&path).await.with_context(|| {
        format!(
            "invalid per-game config — fix the JSON in {}, or delete it to start fresh",
            path.display()
        )
    })
}

pub async fn save_user_configs(configs: &UserConfigStore) -> Result<()> {
    let path = config_dir()?.join("user_apps.json");
    write_json_pretty(&path, configs).await
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
