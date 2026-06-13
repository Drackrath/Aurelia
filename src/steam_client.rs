use crate::cloud_sync::{default_cloud_root, CloudClient, CloudPathResolver, UfsSaveSpec};
use crate::cm_list::get_cm_endpoints;
use crate::config::{
    delete_session, library_cache_path, load_launcher_config, load_library_cache, load_session,
    save_library_cache, save_session,
};
use crate::depot_browser::{self, DepotInfo as BrowserDepotInfo, ManifestFileEntry};
use crate::models::{
    DepotPlatform, DlcState, DownloadProgress, DownloadProgressState, LibraryGame,
    ManifestSelection, OwnedGame, SessionState, SteamGuardReq, UserProfile,
};
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::time::{Duration, Instant};

use steam_vent::auth::{
    AuthConfirmationHandler, ConfirmationMethod, DeviceConfirmationHandler, FileGuardDataStore,
    UserProvidedAuthConfirmationHandler,
};
use steam_vent::connection::Connection;
use steam_vent_proto::steammessages_clientserver::CMsgClientGetAppOwnershipTicket;
use steam_vent_proto::steammessages_clientserver_2::{
    CMsgClientGetDepotDecryptionKey, CMsgClientGetDepotDecryptionKeyResponse,
};
use steam_vent_proto::steammessages_clientserver_appinfo::{
    cmsg_client_picsproduct_info_request, CMsgClientPICSProductInfoRequest,
    CMsgClientPICSProductInfoResponse,
};
use steam_vent_proto::steammessages_contentsystem_steamclient::{
    CContentServerDirectory_GetCDNAuthToken_Request,
    CContentServerDirectory_GetCDNAuthToken_Response,
    CContentServerDirectory_GetManifestRequestCode_Request,
    CContentServerDirectory_GetManifestRequestCode_Response,
    CContentServerDirectory_GetServersForSteamPipe_Request,
    CContentServerDirectory_GetServersForSteamPipe_Response,
};
use steam_vent_proto::steammessages_familygroups_steamclient::{
    CFamilyGroups_GetFamilyGroupForUser_Request, CFamilyGroups_GetFamilyGroupForUser_Response,
    CFamilyGroups_GetSharedLibraryApps_Request, CFamilyGroups_GetSharedLibraryApps_Response,
};
use steam_vent_proto::steammessages_player_steamclient::{
    CPlayer_GetGameAchievements_Request, CPlayer_GetGameAchievements_Response,
    CPlayer_GetOwnedGames_Request, CPlayer_GetOwnedGames_Response,
};
use steam_vent_proto::steammessages_clientserver_userstats::{
    CMsgClientGetUserStats, CMsgClientGetUserStatsResponse,
};
use steam_vent_proto::steammessages_storebrowse_steamclient::{
    CStoreBrowse_GetItems_Request, CStoreBrowse_GetItems_Response, EStoreAppType,
    StoreBrowseContext, StoreBrowseItemDataRequest, StoreItem, StoreItemID,
};
use protobuf::MessageField;
use steam_vent::{ConnectionError, ConnectionTrait, ServerList};
use tokio::io::{duplex, sink, AsyncWriteExt};
use tokio::sync::mpsc::Receiver;

/// How long to wait for a single Steam CM connection/refresh-token logon before
/// giving up on that server. The initial WebSocket handshake has no timeout of
/// its own, so an unresponsive CM would otherwise block the command forever.
const CM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Upper bound on a single liveness probe ([`SteamClient::probe_alive`]). Generous
/// enough not to false-positive when the connection is merely busy (e.g. during a
/// heavy download), short enough to detect a dead socket within one probe cycle.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// How many CM servers to try before failing. `ServerList::pick_ws` rotates
/// round-robin, so each attempt hits a different server.
const CM_CONNECT_ATTEMPTS: usize = 3;

/// Authenticate with a stored refresh token, with a per-attempt timeout and
/// automatic failover to the next CM server.
///
/// `Connection::access` connects to a single WebSocket CM server and performs
/// the refresh-token logon, but it never times out — a stalled CM leaves the
/// whole command hanging (the reported symptom). Each call re-picks a server
/// round-robin, so on timeout/error we simply retry, advancing to the next one.
async fn access_with_retry(
    server_list: &ServerList,
    account: &str,
    refresh_token: &str,
) -> Result<Connection> {
    let mut last_err: Option<anyhow::Error> = None;

    for attempt in 1..=CM_CONNECT_ATTEMPTS {
        tracing::debug!(
            attempt,
            attempts = CM_CONNECT_ATTEMPTS,
            "Authenticating with stored refresh token ..."
        );
        match tokio::time::timeout(
            CM_CONNECT_TIMEOUT,
            Connection::access(server_list, account, refresh_token),
        )
        .await
        {
            Ok(Ok(connection)) => {
                tracing::debug!("Refresh-token authentication succeeded");
                return Ok(connection);
            }
            Ok(Err(err)) => {
                tracing::warn!(
                    "CM connect attempt {attempt}/{CM_CONNECT_ATTEMPTS} failed: {err}"
                );
                last_err = Some(anyhow::Error::new(err));
            }
            Err(_) => {
                tracing::warn!(
                    "CM connect attempt {attempt}/{CM_CONNECT_ATTEMPTS} timed out after {}s; \
                     trying next server",
                    CM_CONNECT_TIMEOUT.as_secs()
                );
                last_err = Some(anyhow!(
                    "Steam CM connection timed out after {}s",
                    CM_CONNECT_TIMEOUT.as_secs()
                ));
            }
        }
    }

    Err(last_err
        .unwrap_or_else(|| anyhow!("failed to connect to any Steam CM server")))
    .context("refresh token login failed")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginState {
    Connected,
    AwaitingCredentialSession,
    AwaitingGuardConfirmation,
    AwaitingPollResult,
    AwaitingAccessTokenLogon,
    Complete,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchTarget {
    NativeLinux,
    WindowsProton,
}

#[derive(Debug, Clone)]
pub struct LaunchInfo {
    pub app_id: u32,
    pub id: String,
    pub description: String,
    pub executable: String,
    pub arguments: String,
    pub workingdir: Option<String>,
    pub target: LaunchTarget,
}

#[derive(Debug, Clone)]
pub struct RawLaunchOption {
    pub executable: String,
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub struct ExtendedAppInfo {
    pub name: Option<String>,
    pub dlcs: Vec<u32>,
    pub depots: Vec<(u32, String)>,
    pub launch_options: Vec<RawLaunchOption>,
    pub active_branch: String,
}

/// Human-facing store metadata for an app, sourced from the `StoreBrowse.GetItems`
/// service method over the Steam CM connection (no HTTPS storefront API). This is
/// the protocol-native replacement for the fields Aurelia previously scraped from
/// `store.steampowered.com/api/appdetails`.
///
/// Note: a few storefront-only fields have no equivalent in this protocol and are
/// intentionally absent — system requirements, Metacritic score, and
/// name-resolved store tags/genres/categories (StoreBrowse returns only numeric
/// ids for those, which would need a separate localized tag dictionary).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StoreAppInfo {
    pub app_id: u32,
    pub name: String,
    pub app_type: String,
    pub is_free: bool,
    pub is_early_access: bool,
    pub short_description: String,
    pub full_description: String,
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    pub franchises: Vec<String>,
    pub release_date: Option<String>,
    pub coming_soon: bool,
    pub price: Option<String>,
    pub discount_pct: i32,
    pub platforms: Vec<String>,
    pub review_summary: Option<String>,
    /// Artwork URLs (header/cover/hero/background/logo).
    pub assets: StoreAppAssets,
}

/// Artwork URLs for a store app. Built from the StoreBrowse `assets` block when
/// present, falling back to Steam's conventional per-appid CDN paths so a caller
/// (e.g. Heroic) always has usable URLs instead of guessing them itself.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StoreAppAssets {
    /// Wide store header / capsule (`header.jpg`).
    pub header: Option<String>,
    /// Portrait library cover (`library_600x900`).
    pub capsule: Option<String>,
    /// Wide library hero background.
    pub hero: Option<String>,
    /// Store-page background.
    pub background: Option<String>,
    /// Transparent game logo (`logo.png`). Not in StoreBrowse — always the
    /// conventional CDN URL.
    pub logo: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AppMetadata {
    pub name: String,
    pub header_image: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DepotInfo {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub file_count: u64,
    pub config: String,
    pub is_owned: Option<bool>,
}

/// A pre-install size estimate for a game on a given platform, derived purely
/// from PICS appinfo (no manifest/CDN fetch). `download_size` is the compressed
/// bytes transferred; `disk_size` is the installed (uncompressed) footprint.
#[derive(Debug, Clone, Default)]
pub struct InstallSizeEstimate {
    pub download_size: u64,
    pub disk_size: u64,
    pub depot_count: u64,
}

/// A single Steam achievement for a game, combining the game's achievement
/// definition (name/description/icons/global rarity) with the logged-in user's
/// unlock state. Shaped to match how launchers (Heroic/Legendary) report
/// achievements.
#[derive(Debug, Clone, Default)]
pub struct GameAchievement {
    /// Stable API/internal name (e.g. `ACH_WIN_ONE_GAME`).
    pub api_name: String,
    /// Localized display name.
    pub name: String,
    /// Localized description (may be empty for hidden achievements).
    pub description: String,
    /// Whether the achievement is hidden until unlocked.
    pub hidden: bool,
    /// Full CDN URL of the unlocked (color) icon.
    pub icon_unlocked: String,
    /// Full CDN URL of the locked (gray) icon.
    pub icon_locked: String,
    /// Global unlock rate across all players, as a percentage (rarity).
    pub global_percent: f32,
    /// Whether the logged-in user has unlocked it.
    pub unlocked: bool,
    /// Unix time (seconds) the user unlocked it, if unlocked.
    pub unlock_time: Option<u32>,
}

/// One entry from a game's PICS `config/launch` table — a way Steam knows to
/// start the game (an executable, its arguments, and platform constraints).
#[derive(Debug, Clone, Default)]
pub struct LaunchOptionInfo {
    /// The launch entry's key (`"0"`, `"1"`, …); `"0"` is the default.
    pub id: String,
    pub description: String,
    pub executable: String,
    pub arguments: String,
    pub working_dir: String,
    /// Target OS (`windows`/`linux`/`macos`), or empty for any.
    pub oslist: String,
    /// Target architecture (`32`/`64`), or empty for any.
    pub osarch: String,
    /// Steam launch `type` (e.g. `default`, `option1`), if set.
    pub launch_type: String,
}

#[derive(Debug, Clone)]
pub struct ConfirmationPrompt {
    pub requirement: SteamGuardReq,
    pub details: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AccountData {
    pub steam_id: u64,
    pub account_name: String,
    pub country: String,      // GeoIP Country
    pub authed_machines: u32, // Steam Guard count
    pub flags: u32,           // Account Flags
    pub email: String,
    pub email_validated: bool,
    pub vac_bans: u32,        // Num VAC bans
    pub vac_banned_apps: Vec<u32>,
}

/// A game available to the account through Steam Family Sharing.
#[derive(Debug, Clone)]
pub struct SharedApp {
    pub app_id: u32,
    pub name: String,
    /// SteamID64 of the family member who owns (lends) this app, if reported.
    pub owner_steamid: Option<u64>,
}

#[derive(Clone)]
pub struct SteamClient {
    connection: Option<Connection>,
    state: LoginState,
    connected_at: Option<Instant>,
    active_cm: Option<SocketAddr>,
    server_list: Option<ServerList>,
    pending_confirmations: Vec<ConfirmationPrompt>,
}

// SteamClient methods are implemented across these submodules.
mod client;
mod chat;
mod friends;
mod install;
mod manage;
mod content;
mod library;
mod launch;
mod manifests;
mod process;
mod workshop;
mod workshop_manifest;

pub use friends::{resolve_steam_id, AddedFriend, Friend, ResolvedUser, Roster};

/// Terminate the process with `pid` and any children it spawned.
///
/// On Windows the launched process is often a thin launcher that re-spawns the
/// real game, so we kill the whole tree with `taskkill /T`. On Unix we send
/// `SIGTERM` then `SIGKILL` to give the game a chance to exit cleanly first.
fn kill_process_tree(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }

    #[cfg(unix)]
    {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        std::thread::sleep(Duration::from_millis(500));
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }
}

pub fn sanitize_install_dir(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            #[cfg(target_os = "windows")]
            ':' => '_',
            _ => c,
        })
        .collect();
    sanitized.trim().to_string()
}

/// Steam wraps the entire VDF in a top-level key that is the numeric app ID.
/// This wrapper accepts that outer key transparently.
#[derive(Debug, serde::Deserialize)]
pub struct AppInfoEnvelope(pub HashMap<String, crate::models::AppInfoRoot>);

impl AppInfoEnvelope {
    /// Extract the inner AppInfoRoot regardless of the outer key name.
    pub fn into_inner(self) -> Option<crate::models::AppInfoRoot> {
        self.0.into_values().next()
    }
}

/// Read a value from `HKCU\Software\Valve\Steam` via `reg query` (Windows).
#[cfg(target_os = "windows")]
fn read_steam_registry(value: &str) -> Option<String> {
    let output = Command::new("reg")
        .args(["query", r"HKCU\Software\Valve\Steam", "/v", value])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with(value) {
            // Line looks like: `SteamExe    REG_SZ    c:/program files (x86)/steam/steam.exe`
            if let Some(pos) = line.find("REG_") {
                let mut parts = line[pos..].splitn(2, char::is_whitespace);
                let _ty = parts.next();
                if let Some(data) = parts.next() {
                    return Some(data.trim().to_string());
                }
            }
        }
    }
    None
}

/// Resolve the path to `steam.exe` (Windows).
#[cfg(target_os = "windows")]
fn steam_exe_path() -> Option<PathBuf> {
    if let Some(raw) = read_steam_registry("SteamExe") {
        let path = PathBuf::from(raw.replace('/', "\\"));
        if path.exists() {
            return Some(path);
        }
    }
    let fallback = PathBuf::from(r"C:\Program Files (x86)\Steam\steam.exe");
    fallback.exists().then_some(fallback)
}

/// Add or remove a DLC appid from every `DisabledDLC` list in an appmanifest's text.
/// When `disabled` is true and no `DisabledDLC` key exists, one is inserted into the
/// `UserConfig` and `MountedConfig` blocks.
/// Collect the DLC appids whose content is recorded as installed in an appmanifest.
/// Steam tags each DLC depot under `InstalledDepots` with a `"dlcappid" "<id>"` line.
fn parse_installed_dlc_appids(content: &str) -> HashSet<u32> {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#""dlcappid"\s*"(\d+)""#).unwrap());
    RE.captures_iter(content)
        .filter_map(|caps| caps[1].parse::<u32>().ok())
        .collect()
}

/// Collect the DLC appids listed in an appmanifest's `DisabledDLC` value(s).
/// The value is a comma-separated list of appids; multiple blocks may each carry one.
fn parse_disabled_dlc_appids(content: &str) -> HashSet<u32> {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#""DisabledDLC"\s*"([^"]*)""#).unwrap());
    RE.captures_iter(content)
        .flat_map(|caps| {
            caps[1]
                .split(',')
                .filter_map(|s| s.trim().parse::<u32>().ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn apply_dlc_disabled(content: &str, dlc_appid: u32, disabled: bool) -> String {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#""DisabledDLC"(\s*)"([^"]*)""#).unwrap());
    let dlc = dlc_appid.to_string();

    if RE.is_match(content) {
        return RE
            .replace_all(content, |caps: &regex::Captures| {
                let mut list: Vec<String> = caps[2]
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty() && *s != dlc)
                    .collect();
                if disabled {
                    list.push(dlc.clone());
                }
                format!("\"DisabledDLC\"{}\"{}\"", &caps[1], list.join(","))
            })
            .into_owned();
    }

    if disabled {
        // No DisabledDLC key yet — add one to the per-user config blocks.
        let mut out = content.to_string();
        for block in ["MountedConfig", "UserConfig"] {
            if let Some(pos) = out.find(&format!("\"{block}\"")) {
                if let Some(rel) = out[pos..].find('{') {
                    let at = pos + rel + 1;
                    out.insert_str(at, &format!("\n\t\t\"DisabledDLC\"\t\t\"{dlc}\""));
                }
            }
        }
        return out;
    }

    content.to_string()
}

/// Collect the non-empty display names from a list of `CreatorHomeLink`s
/// (developers / publishers / franchises in a `StoreItem.basic_info`).
fn creator_names(
    creators: &[steam_vent_proto::steammessages_storebrowse_steamclient::store_item::basic_info::CreatorHomeLink],
) -> Vec<String> {
    creators
        .iter()
        .map(|c| c.name().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Map a `StoreBrowse` `StoreItem` protobuf into our display-oriented
/// [`StoreAppInfo`]. Defensive throughout: every nested message is optional, so
/// missing data yields empty fields rather than failing.
fn store_item_to_app_info(item: &StoreItem) -> StoreAppInfo {
    let basic = item.basic_info.as_ref();

    let release = item.release.as_ref();
    let release_date = release.and_then(|r| {
        let custom = r.custom_release_date_message();
        if !custom.is_empty() {
            Some(custom.to_string())
        } else if r.steam_release_date() > 0 {
            Some(unix_to_ymd(r.steam_release_date() as i64))
        } else {
            None
        }
    });

    let platforms = item
        .platforms
        .as_ref()
        .map(|p| {
            let mut v = Vec::new();
            if p.windows() {
                v.push("Windows".to_string());
            }
            if p.mac() {
                v.push("macOS".to_string());
            }
            if p.steamos_linux() {
                v.push("Linux".to_string());
            }
            v
        })
        .unwrap_or_default();

    let (price, discount_pct) = match item.best_purchase_option.as_ref() {
        _ if item.is_free() => (Some("Free".to_string()), 0),
        Some(p) if !p.formatted_final_price().is_empty() => {
            (Some(p.formatted_final_price().to_string()), p.discount_pct())
        }
        _ => (None, 0),
    };

    let review_summary = item
        .reviews
        .as_ref()
        .and_then(|r| r.summary_filtered.as_ref())
        .and_then(|s| {
            let label = s.review_score_label();
            if label.is_empty() {
                None
            } else {
                Some(format!(
                    "{} ({}% positive, {} reviews)",
                    label,
                    s.percent_positive(),
                    s.review_count()
                ))
            }
        });

    StoreAppInfo {
        app_id: item.appid(),
        name: item.name().to_string(),
        app_type: store_app_type_label(item.type_()),
        is_free: item.is_free(),
        is_early_access: item.is_early_access(),
        short_description: basic.map(|b| b.short_description().to_string()).unwrap_or_default(),
        full_description: crate::store::strip_html(item.full_description()),
        developers: basic
            .map(|b| creator_names(&b.developers))
            .unwrap_or_default(),
        publishers: basic
            .map(|b| creator_names(&b.publishers))
            .unwrap_or_default(),
        franchises: basic
            .map(|b| creator_names(&b.franchises))
            .unwrap_or_default(),
        release_date,
        coming_soon: release.map(|r| r.is_coming_soon()).unwrap_or(false),
        price,
        discount_pct,
        platforms,
        review_summary,
        assets: store_assets(item),
    }
}

/// Resolve a `StoreItem`'s artwork URLs. Each is built from the StoreBrowse
/// `assets` block (`asset_url_format` with the asset filename substituted for
/// `${FILENAME}`) when available, otherwise from Steam's conventional per-appid
/// CDN paths so the caller always gets a usable URL.
fn store_assets(item: &StoreItem) -> StoreAppAssets {
    let appid = item.appid();
    let assets = item.assets.as_ref();
    let fmt = assets.map(|a| a.asset_url_format()).unwrap_or("");

    let from_fmt = |filename: &str| -> Option<String> {
        if filename.is_empty() || fmt.is_empty() {
            return None;
        }
        let path = fmt.replace("${FILENAME}", filename);
        if path.starts_with("http") {
            Some(path)
        } else {
            Some(format!(
                "https://shared.cloudflare.steamstatic.com/store_item_assets/{path}"
            ))
        }
    };
    let cdn = |file: &str| format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/{file}");
    let pick = |native: Option<String>, fallback: &str| native.or_else(|| Some(cdn(fallback)));

    StoreAppAssets {
        header: pick(assets.and_then(|a| from_fmt(a.header())), "header.jpg"),
        capsule: pick(
            assets.and_then(|a| from_fmt(a.library_capsule_2x())),
            "library_600x900_2x.jpg",
        ),
        hero: pick(
            assets.and_then(|a| from_fmt(a.library_hero_2x())),
            "library_hero.jpg",
        ),
        background: pick(
            assets.and_then(|a| from_fmt(a.page_background())),
            "page_bg_generated_v6b.jpg",
        ),
        // StoreBrowse has no logo field; use the conventional CDN URL.
        logo: Some(cdn("logo.png")),
    }
}

/// Human-readable label for a `StoreBrowse` app type.
fn store_app_type_label(t: EStoreAppType) -> String {
    match t {
        EStoreAppType::k_EStoreAppType_Game => "Game",
        EStoreAppType::k_EStoreAppType_Demo => "Demo",
        EStoreAppType::k_EStoreAppType_Mod => "Mod",
        EStoreAppType::k_EStoreAppType_Movie => "Movie",
        EStoreAppType::k_EStoreAppType_DLC => "DLC",
        EStoreAppType::k_EStoreAppType_Guide => "Guide",
        EStoreAppType::k_EStoreAppType_Software => "Software",
        EStoreAppType::k_EStoreAppType_Video => "Video",
        EStoreAppType::k_EStoreAppType_Series => "Series",
        EStoreAppType::k_EStoreAppType_Episode => "Episode",
        EStoreAppType::k_EStoreAppType_Hardware => "Hardware",
        EStoreAppType::k_EStoreAppType_Music => "Soundtrack",
        EStoreAppType::k_EStoreAppType_Beta => "Beta",
        EStoreAppType::k_EStoreAppType_Tool => "Tool",
        EStoreAppType::k_EStoreAppType_Advertising => "Advertising",
    }
    .to_string()
}

/// Convert a Unix timestamp (seconds, UTC) to a `YYYY-MM-DD` date string using
/// Howard Hinnant's days-from-civil algorithm. Avoids pulling in a date crate.
pub fn unix_to_ymd(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    // Shift epoch to 0000-03-01 so leap days fall at the end of the era.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

pub fn parse_appinfo(vdf: &str) -> Result<crate::models::AppInfoRoot> {
    // Try direct parse first (in case steam-vent already strips the wrapper)
    if let Ok(parsed) = keyvalues_serde::from_str::<crate::models::AppInfoRoot>(vdf) {
        return Ok(parsed);
    }
    // Fall back to envelope parse
    let envelope: AppInfoEnvelope =
        keyvalues_serde::from_str(vdf).context("failed parsing appinfo VDF (envelope)")?;
    envelope
        .into_inner()
        .context("appinfo envelope was empty")
}

/// Steam store category IDs that denote *online* multiplayer (as opposed to
/// local/LAN play). A title carrying any of these effectively needs a network
/// connection for its core multiplayer experience.
/// 20 = MMO, 36 = Online PvP, 38 = Online Co-op.
const ONLINE_MULTIPLAYER_CATEGORIES: &[u32] = &[20, 36, 38];
/// Steam store category ID for Single-player.
const SINGLE_PLAYER_CATEGORY: u32 = 2;

/// Infer whether a game requires an online connection from its PICS `common`
/// store-category map (keyed `category_<id>`).
///
/// Steam has no dedicated "online required" field, so this is a heuristic: a
/// title is treated as online-required when it advertises an online-multiplayer
/// category (MMO / Online PvP / Online Co-op) but does *not* advertise
/// single-player support — i.e. there is no documented way to play it offline.
pub fn category_online_required(categories: &HashMap<String, String>) -> bool {
    let has = |id: u32| {
        categories
            .get(&format!("category_{id}"))
            .map(|v| v != "0")
            .unwrap_or(false)
    };
    let online = ONLINE_MULTIPLAYER_CATEGORIES.iter().any(|&id| has(id));
    online && !has(SINGLE_PLAYER_CATEGORY)
}

/// Best-effort: relocate app `appid`'s entry from `from_lib` to `to_lib` in
/// `libraryfolders.vdf` (or just add it when `from_lib == to_lib`, e.g. for an
/// import). Logs and continues on any problem — Steam reconciles the index from
/// the appmanifests on its next launch.
async fn update_libraryfolders_for(from_lib: &Path, to_lib: &Path, appid: u32, install_dir: &Path) {
    let roots = crate::library::all_library_roots().await;
    let Some(vdf_path) = crate::relocate::find_libraryfolders_vdf(&roots) else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&vdf_path) else {
        tracing::warn!("could not read libraryfolders.vdf");
        return;
    };
    let size = crate::relocate::dir_size(install_dir);
    match crate::relocate::update_libraryfolders_apps(&text, appid, from_lib, to_lib, size) {
        Some(updated) => {
            if let Err(e) = std::fs::write(&vdf_path, updated) {
                tracing::warn!("could not write libraryfolders.vdf: {e}");
            }
        }
        None => tracing::warn!(
            "could not locate the library entry in libraryfolders.vdf; Steam will reconcile on next launch"
        ),
    }
}

/// Build the full Steam Community CDN URL for an achievement icon hash. Returns
/// it unchanged if it's already a URL, and empty for an empty hash.
fn achievement_icon_url(appid: u32, icon: &str) -> String {
    if icon.is_empty() {
        String::new()
    } else if icon.starts_with("http") {
        icon.to_string()
    } else {
        format!(
            "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/{appid}/{icon}"
        )
    }
}

/// Parse the binary-KV achievement schema from a `ClientGetUserStats` response
/// into a map of achievement api-name → `(stat_id, bit)`, so the user's unlock
/// blocks (keyed by stat id, indexed by bit) can be matched to each achievement.
///
/// Schema shape: `<appid> { stats { <statid> { type "4" bits { <bit> { name
/// "ACH_…" … } } } } }` — `type == 4` marks an achievement bitfield stat.
fn parse_achievement_schema(schema: &[u8]) -> HashMap<String, (u32, u32)> {
    if schema.is_empty() {
        return HashMap::new();
    }
    // The binary-KV parser recurses one frame per nesting level; a malformed or
    // misaligned blob can recurse on a run of zero bytes and overflow the stack.
    // Parse on a thread with a generous stack so it can never crash the process.
    let bytes = schema.to_vec();
    std::thread::Builder::new()
        .name("ach-schema-parse".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || parse_achievement_schema_inner(&bytes))
        .ok()
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default()
}

/// Inner schema parse. Parses the blob directly as binary KeyValues (no
/// appinfo-style offset scanning, which could feed the parser misaligned data).
fn parse_achievement_schema_inner(schema: &[u8]) -> HashMap<String, (u32, u32)> {
    let mut map = HashMap::new();
    let Ok(vdf) = steam_vdf_parser::parse_binary(schema) else {
        return map;
    };
    let Some(root) = vdf.as_obj() else {
        return map;
    };
    // `stats` is directly under the appid-keyed root (or one level deeper).
    let stats = root
        .get("stats")
        .and_then(|v| v.as_obj())
        .or_else(|| {
            root.values()
                .next()
                .and_then(|v| v.as_obj())
                .and_then(|o| o.get("stats"))
                .and_then(|v| v.as_obj())
        });
    let Some(stats) = stats else {
        return map;
    };

    for (stat_key, stat_val) in stats.iter() {
        let Ok(stat_id) = stat_key.parse::<u32>() else {
            continue;
        };
        let Some(stat_obj) = stat_val.as_obj() else {
            continue;
        };
        // Achievement-bitfield stats are exactly those carrying a `bits` table.
        // (Don't filter on `type`: in binary KV it's an integer, not "4".)
        let Some(bits) = stat_obj.get("bits").and_then(|v| v.as_obj()) else {
            continue;
        };
        for (bit_key, bit_val) in bits.iter() {
            let Ok(bit) = bit_key.parse::<u32>() else {
                continue;
            };
            if let Some(name) = bit_val
                .as_obj()
                .and_then(|o| o.get("name"))
                .and_then(|v| v.as_str())
            {
                map.insert(name.to_string(), (stat_id, bit));
            }
        }
    }
    map
}

pub fn should_keep_depot(oslist: Option<&str>, target: DepotPlatform) -> bool {
    match target {
        DepotPlatform::Windows => match oslist {
            Some(os) => {
                let os = os.to_lowercase();
                if os.contains("windows") {
                    return true;
                }
                if os.contains("linux") || os.contains("macos") {
                    return false;
                }
                true
            }
            None => true,
        },
        DepotPlatform::Linux => match oslist {
            Some(os) => {
                let os = os.to_lowercase();
                if os.contains("linux") {
                    return true;
                }
                if os.contains("windows") || os.contains("macos") {
                    return false;
                }
                true
            }
            None => true,
        },
    }
}

fn map_confirmation(method: &ConfirmationMethod) -> ConfirmationPrompt {
    let details = method.confirmation_details().to_string();
    let requirement = match method.confirmation_type() {
        "email" => SteamGuardReq::EmailCode {
            domain_hint: details.clone(),
        },
        "device code" => SteamGuardReq::DeviceCode,
        "device confirmation" => SteamGuardReq::DeviceConfirmation,
        _ => SteamGuardReq::DeviceConfirmation,
    };

    ConfirmationPrompt {
        requirement,
        details,
    }
}

#[derive(Debug, Deserialize)]
struct ProductInfoEnvelopeWrapper(pub HashMap<String, ProductInfoEnvelope>);

impl ProductInfoEnvelopeWrapper {
    pub fn into_inner(self) -> Option<ProductInfoEnvelope> {
        self.0.into_values().next()
    }
}

fn parse_product_info_envelope(vdf: &str) -> Result<ProductInfoEnvelope> {
    if let Ok(parsed) = keyvalues_serde::from_str::<ProductInfoEnvelope>(vdf) {
        return Ok(parsed);
    }
    let wrapper: ProductInfoEnvelopeWrapper = keyvalues_serde::from_str(vdf)
        .context("failed parsing product info VDF (wrapper)")?;
    wrapper
        .into_inner()
        .context("product info envelope was empty")
}

fn parse_launch_info_from_vdf(appid: u32, raw_vdf: &str) -> Result<Vec<LaunchInfo>> {
    let parsed: ProductInfoEnvelope =
        parse_product_info_envelope(raw_vdf).context("failed to parse product info VDF")?;

    let config = parsed
        .appinfo
        .as_ref()
        .and_then(|appinfo| appinfo.config.as_ref())
        .or(parsed.config.as_ref())
        .ok_or_else(|| anyhow!("missing config section in product info for app {appid}"))?;

    if config.launch.is_empty() {
        bail!("no launch entries found for app {appid}")
    }

    let mut options = Vec::new();
    for (id, entry) in &config.launch {
        let exe = entry.executable.as_deref().unwrap_or("");
        let os_list = entry.config.as_ref().and_then(|c| c.oslist.as_deref());
        let description = entry.description.as_deref().unwrap_or("Game");

        // Pick the launch target from the entry's oslist, falling back to the
        // executable extension and finally the host OS.
        let target = if let Some(os) = os_list {
            if os.contains("linux") {
                LaunchTarget::NativeLinux
            } else if os.contains("windows") {
                LaunchTarget::WindowsProton
            } else if os.contains("macos") {
                continue; // we don't launch macOS builds
            } else {
                LaunchTarget::WindowsProton
            }
        } else {
            if exe.ends_with(".exe") || exe.ends_with(".bat") {
                LaunchTarget::WindowsProton
            } else if exe.contains("linux") || exe.ends_with(".sh") {
                LaunchTarget::NativeLinux
            } else {
                #[cfg(target_os = "linux")]
                {
                    LaunchTarget::NativeLinux
                }
                #[cfg(target_os = "windows")]
                {
                    LaunchTarget::WindowsProton
                }
                #[cfg(not(any(target_os = "linux", target_os = "windows")))]
                {
                    LaunchTarget::WindowsProton
                }
            }
        };

        options.push(LaunchInfo {
            app_id: appid,
            id: id.clone(),
            description: if description == "Game" && !exe.is_empty() {
                exe.to_string()
            } else {
                description.to_string()
            },
            executable: exe.to_string(),
            arguments: entry.arguments.clone().unwrap_or_default(),
            workingdir: entry.workingdir.clone(),
            target,
        });
    }

    if options.is_empty() {
        bail!("no suitable launch option found for app {appid}");
    }

    // Sort options: prefer key "0", then by id
    options.sort_by(|a, b| {
        if a.id == "0" {
            return std::cmp::Ordering::Less;
        }
        if b.id == "0" {
            return std::cmp::Ordering::Greater;
        }
        a.id.cmp(&b.id)
    });

    Ok(options)
}

pub fn find_vdf_in_pics(buffer: &[u8]) -> Result<steam_vdf_parser::Vdf<'static>> {
    let is_text = buffer
        .first()
        .map(|&b| b == 0x22 || b == 0x7B)
        .unwrap_or(false);

    if is_text {
        let text = String::from_utf8_lossy(buffer);
        return steam_vdf_parser::parse_text(&text)
            .map(|v| v.into_owned())
            .map_err(|e| anyhow!("Text VDF parse error: {}", e));
    }

    if let Ok(vdf) = steam_vdf_parser::parse_binary(buffer) {
        return Ok(vdf.into_owned());
    }

    for offset in 1..std::cmp::min(128, buffer.len()) {
        if let Ok(vdf) = steam_vdf_parser::parse_binary(&buffer[offset..]) {
            tracing::info!("Success! Found VDF at offset {}", offset);
            return Ok(vdf.into_owned());
        }
    }

    bail!("Failed to locate valid VDF (Text or Binary) in PICS buffer")
}

/// PICS appinfo nests an app's sections (`common`, `extended`, `config`, `depots`)
/// under a single root key — either the literal `appinfo` or the numeric appid.
/// Return the inner value that actually holds those sections, descending one level
/// past the wrapper when present so callers can navigate by section name directly.
pub(crate) fn pics_app_section<'a>(
    root: &'a steam_vdf_parser::Value<'static>,
) -> &'a steam_vdf_parser::Value<'static> {
    fn has_sections(v: &steam_vdf_parser::Value) -> bool {
        ["common", "extended", "config", "depots"]
            .iter()
            .any(|k| v.get(k).is_some())
    }
    if has_sections(root) {
        return root;
    }
    if let Some(inner) = root.as_obj().and_then(|o| o.values().find(|v| has_sections(v))) {
        return inner;
    }
    root
}

/// Steam Auto-Cloud save-file rules from an app's `ufs/savefiles` PICS section.
/// Each entry names a root token, sub-path, filename glob, and recursion flag.
pub(crate) fn ufs_save_specs_from_section(section: &steam_vdf_parser::Value) -> Vec<UfsSaveSpec> {
    let mut specs = Vec::new();
    if let Some(savefiles) = section.get_obj(&["ufs", "savefiles"]) {
        for (_, entry) in savefiles.iter() {
            let get = |k: &str| entry.get(k).and_then(|v| v.as_str());
            let root = get("root").unwrap_or("").to_string();
            if root.is_empty() {
                continue;
            }
            specs.push(UfsSaveSpec {
                root,
                path: get("path").unwrap_or("").to_string(),
                pattern: get("pattern").unwrap_or("*").to_string(),
                recursive: get("recursive") == Some("1"),
            });
        }
    }
    specs
}

/// DLC app ids declared by an app's PICS section, read from the canonical
/// `extended/listofdlc` field (a comma-separated string of app ids). Order is
/// preserved and duplicates dropped.
pub(crate) fn dlc_ids_from_section(section: &steam_vdf_parser::Value) -> Vec<u32> {
    let mut dlcs: Vec<u32> = Vec::new();
    if let Some(list) = section.get_str(&["extended", "listofdlc"]) {
        for id in list.split(',').filter_map(|p| p.trim().parse::<u32>().ok()) {
            if !dlcs.contains(&id) {
                dlcs.push(id);
            }
        }
    }
    dlcs
}

pub fn parse_pics_product_info(buffer: &[u8]) -> Result<HashMap<u64, u64>> {
    let is_text = buffer
        .first()
        .map(|&b| b == 0x22 || b == 0x7B)
        .unwrap_or(false);

    if is_text {
        parse_text_vdf(buffer)
    } else {
        parse_binary_vdf_with_offset(buffer)
    }
}

fn parse_text_vdf(data: &[u8]) -> Result<HashMap<u64, u64>> {
    let text = String::from_utf8_lossy(data);
    let mut depot_map = HashMap::new();

    match steam_vdf_parser::parse_text(&text) {
        Ok(vdf) => {
            let root_obj = vdf.as_obj().unwrap();
            let depots_val = root_obj.get("depots").or_else(|| {
                root_obj
                    .get("appinfo")
                    .and_then(|v| v.as_obj())
                    .and_then(|o| o.get("depots"))
            });

            if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
                for (key, value) in depots.iter() {
                    if let Ok(depot_id) = key.parse::<u64>() {
                        // Language check for library-parsed VDF
                        let lang = value
                            .get_obj(&["config"])
                            .and_then(|c| c.get("language"))
                            .and_then(|l| l.as_str());
                        if let Some(lang) = lang {
                            if lang != "english" && !lang.is_empty() {
                                continue;
                            }
                        }

                        if let Some(m_id) = extract_manifest_id_robust(value, "public") {
                            depot_map.insert(depot_id, m_id);
                        }
                    }
                }
            }
        }
        Err(_) => {}
    }

    if depot_map.is_empty() {
        let mut current_depot = 0;
        let mut inside_depots = false;
        let mut inside_manifests = false;
        let mut inside_public = false;
        let mut depot_langs = HashMap::new();

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.contains("\"depots\"") {
                inside_depots = true;
                continue;
            }
            if !inside_depots {
                continue;
            }

            if trimmed == "}" {
                if inside_public {
                    inside_public = false;
                } else if inside_manifests {
                    inside_manifests = false;
                }
                continue;
            }

            if trimmed.starts_with("\"manifests\"") {
                inside_manifests = true;
                continue;
            }
            if inside_manifests && trimmed.starts_with("\"public\"") {
                inside_public = true;
                continue;
            }

            let parts = extract_quoted_values(trimmed);
            if parts.len() == 1 {
                if let Ok(id) = parts[0].parse::<u64>() {
                    current_depot = id;
                    inside_manifests = false;
                    inside_public = false;
                }
            } else if parts.len() >= 2 && current_depot > 0 {
                let key = parts[0].to_lowercase();
                if inside_public && key == "gid" {
                    if let Ok(gid) = parts[1].parse::<u64>() {
                        if gid > 0 {
                            depot_map.insert(current_depot, gid);
                        }
                    }
                } else if key == "language" {
                    depot_langs.insert(current_depot, parts[1].to_lowercase());
                } else if !inside_manifests && (key == "manifest" || key == "gid") {
                    // Fallback for flat structure
                    if let Ok(gid) = parts[1].parse::<u64>() {
                        if gid > 0 {
                            depot_map.insert(current_depot, gid);
                        }
                    }
                }
            }
        }

        // Apply Language Filter to manual scan results
        depot_map.retain(|id, _| {
            if let Some(lang) = depot_langs.get(id) {
                if lang != "english" && !lang.is_empty() {
                    return false;
                }
            }
            true
        });
    }

    if depot_map.is_empty() {
        bail!("Text scan found no depots");
    }

    Ok(depot_map)
}

fn parse_binary_vdf_with_offset(data: &[u8]) -> Result<HashMap<u64, u64>> {
    if let Ok(vdf) = find_vdf_in_pics(data) {
        let mut depot_map = HashMap::new();
        let root_obj = vdf.as_obj().context("root is not an object")?;
        let depots_val = root_obj.get("depots").or_else(|| {
            root_obj
                .get("appinfo")
                .and_then(|v| v.as_obj())
                .and_then(|o| o.get("depots"))
        });

        if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
            for (key, value) in depots.iter() {
                if let Ok(depot_id) = key.parse::<u64>() {
                    // Language check for binary-parsed VDF
                    let lang = value
                        .get_obj(&["config"])
                        .and_then(|c| c.get("language"))
                        .and_then(|l| l.as_str());
                    if let Some(lang) = lang {
                        if lang != "english" && !lang.is_empty() {
                            continue;
                        }
                    }

                    if let Some(m_id) = extract_manifest_id_robust(value, "public") {
                        depot_map.insert(depot_id, m_id);
                    }
                }
            }
        }

        if !depot_map.is_empty() {
            return Ok(depot_map);
        }
    }
    bail!("Failed to locate valid Binary VDF in PICS buffer")
}

pub fn parse_depots_robust(data: &[u8]) -> Result<HashMap<u64, u64>> {
    parse_pics_product_info(data)
}

fn extract_manifest_id_robust(value: &steam_vdf_parser::Value, branch: &str) -> Option<u64> {
    if let Some(obj) = value.as_obj() {
        // Deep search for branch manifest
        if let Some(manifests) = obj.get("manifests").and_then(|v| v.as_obj()) {
            if let Some(branch_entry) = manifests.get(branch) {
                // It can be a direct string or a gid object
                if let Some(gid_str) = branch_entry.as_str() {
                    if let Ok(gid) = gid_str.parse::<u64>() {
                        return Some(gid);
                    }
                }
                if let Some(gid_val) = branch_entry.as_u64() {
                    return Some(gid_val);
                }
                if let Some(branch_obj) = branch_entry.as_obj() {
                    if let Some(gid) = branch_obj.get("gid") {
                        if let Some(s) = gid.as_str() {
                            return s.parse().ok();
                        }
                        return gid.as_u64();
                    }
                }
            }
        }

        // Direct gid
        if let Some(gid_entry) = obj.get("gid") {
            if let Some(gid_str) = gid_entry.as_str() {
                return gid_str.parse::<u64>().ok();
            }
            if let Some(gid_val) = gid_entry.as_u64() {
                return Some(gid_val);
            }
        }
    }

    None
}

#[derive(Debug, Deserialize)]
struct ProductInfoEnvelope {
    #[serde(default)]
    appinfo: Option<ProductInfoAppInfo>,
    #[serde(default)]
    config: Option<ProductInfoConfig>,
}

#[derive(Debug, Deserialize)]
struct ProductInfoAppInfo {
    #[serde(default)]
    config: Option<ProductInfoConfig>,
}

#[derive(Debug, Deserialize)]
struct ProductInfoConfig {
    #[serde(default)]
    launch: HashMap<String, ProductLaunchEntry>,
}

#[derive(Debug, Deserialize)]
struct ProductLaunchEntry {
    #[serde(default)]
    pub executable: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub workingdir: Option<String>,
    #[serde(default)]
    pub config: Option<ProductLaunchConfigInner>,
}

#[derive(Debug, Deserialize)]
struct ProductLaunchConfigInner {
    #[serde(default)]
    oslist: Option<String>,
}

fn parse_installdir_from_acf(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let quoted = extract_quoted_values(line.trim());
        if quoted.len() >= 2 && quoted[0] == "installdir" {
            return Some(quoted[1].clone());
        }
    }
    None
}
fn parse_installed_depots_from_acf(raw: &str) -> HashMap<u64, u64> {
    let mut manifests = HashMap::new();
    let mut in_installed_depots = false;
    let mut current_depot: Option<u64> = None;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.contains("\"InstalledDepots\"") {
            in_installed_depots = true;
            continue;
        }

        if !in_installed_depots {
            continue;
        }

        if trimmed == "}" {
            if current_depot.is_some() {
                current_depot = None;
                continue;
            }
            break;
        }

        let quoted = extract_quoted_values(trimmed);
        if quoted.len() == 1 {
            if let Ok(depot_id) = u64::from_str(&quoted[0]) {
                current_depot = Some(depot_id);
            }
        } else if quoted.len() >= 2 && quoted[0] == "manifest" && current_depot.is_some() {
            if let Ok(manifest) = u64::from_str(&quoted[1]) {
                manifests.insert(current_depot.unwrap_or_default(), manifest);
            }
        }
    }

    manifests
}

fn parse_active_branch_from_acf(raw: &str) -> String {
    let mut in_user_config = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        let parts = extract_quoted_values(trimmed);

        if parts.len() == 1 && parts[0].eq_ignore_ascii_case("userconfig") {
            in_user_config = true;
            continue;
        }

        if trimmed == "{" || trimmed == "}" {
            if trimmed == "}" && in_user_config {
                in_user_config = false;
            }
            continue;
        }

        if parts.len() >= 2 && in_user_config && parts[0].eq_ignore_ascii_case("betakey") {
            if !parts[1].trim().is_empty() {
                return parts[1].to_string();
            }
        }
    }
    "public".to_string()
}


fn rewrite_app_branch(raw: &str, branch: &str) -> String {
    let mut out = Vec::new();
    let mut in_user_config = false;
    let mut branch_updated = false;

    for line in raw.lines() {
        let trimmed = line.trim();

        if trimmed.eq_ignore_ascii_case("\"UserConfig\"") {
            in_user_config = true;
            out.push(line.to_string());
            continue;
        }

        if in_user_config && trimmed == "{" {
            out.push(line.to_string());
            continue;
        }

        if in_user_config && trimmed == "}" {
            if !branch_updated {
                out.push(format!("\t\t\"BetaKey\"\t\t\"{branch}\""));
            }
            in_user_config = false;
            out.push(line.to_string());
            continue;
        }

        if in_user_config {
            let quoted = extract_quoted_values(trimmed);
            if !quoted.is_empty() && quoted[0].eq_ignore_ascii_case("betakey") {
                let indent = line
                    .chars()
                    .take_while(|ch| ch.is_whitespace())
                    .collect::<String>();
                out.push(format!("{indent}\"BetaKey\"\t\t\"{branch}\""));
                branch_updated = true;
                continue;
            }
        }

        out.push(line.to_string());
    }

    // If UserConfig was never found, we might need to add it, but for simplicity
    // we assume it exists in a valid Steam manifest.

    out.join("\n")
}

fn extract_quoted_values(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_quote = false;
    let mut current = String::new();
    for ch in line.chars() {
        if ch == '"' {
            if in_quote {
                out.push(current.clone());
                current.clear();
            }
            in_quote = !in_quote;
            continue;
        }
        if in_quote {
            current.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cats(pairs: &[(u32, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(id, v)| (format!("category_{id}"), v.to_string()))
            .collect()
    }

    #[test]
    fn online_required_mmo_without_single_player() {
        // MMO (20), no single-player => requires online.
        assert!(category_online_required(&cats(&[(20, "1"), (1, "1")])));
    }

    #[test]
    fn online_required_online_coop_without_single_player() {
        // Online Co-op (38) only => requires online.
        assert!(category_online_required(&cats(&[(38, "1")])));
    }

    #[test]
    fn not_online_required_when_single_player_present() {
        // Online PvP (36) but also Single-player (2) => playable offline.
        assert!(!category_online_required(&cats(&[(36, "1"), (2, "1")])));
    }

    #[test]
    fn not_online_required_for_local_multiplayer_only() {
        // Generic Multi-player (1) / Shared-Split-Screen (24) are not online-only.
        assert!(!category_online_required(&cats(&[(1, "1"), (24, "1")])));
    }

    #[test]
    fn not_online_required_when_categories_absent_or_zeroed() {
        assert!(!category_online_required(&cats(&[])));
        assert!(!category_online_required(&cats(&[(20, "0"), (2, "0")])));
    }

    #[test]
    fn unix_to_ymd_known_dates() {
        assert_eq!(unix_to_ymd(0), "1970-01-01");
        assert_eq!(unix_to_ymd(1_700_000_000), "2023-11-14"); // 2023-11-14T22:13:20Z
        assert_eq!(unix_to_ymd(1_009_843_200), "2002-01-01"); // exact midnight UTC
        // Leap day round-trips correctly.
        assert_eq!(unix_to_ymd(1_582_934_400), "2020-02-29");
    }

    #[test]
    fn achievement_icon_urls() {
        assert_eq!(achievement_icon_url(440, ""), "");
        assert_eq!(
            achievement_icon_url(440, "abc123.jpg"),
            "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/440/abc123.jpg"
        );
        // An already-absolute URL is passed through unchanged.
        assert_eq!(
            achievement_icon_url(440, "https://example.com/i.png"),
            "https://example.com/i.png"
        );
    }

    #[test]
    fn store_app_type_labels() {
        assert_eq!(store_app_type_label(EStoreAppType::k_EStoreAppType_Game), "Game");
        assert_eq!(store_app_type_label(EStoreAppType::k_EStoreAppType_DLC), "DLC");
        assert_eq!(store_app_type_label(EStoreAppType::k_EStoreAppType_Music), "Soundtrack");
    }

    #[tokio::test]
    async fn test_legacy_path_blocks_windows_proton() {
        let client = SteamClient::new().unwrap();
        let app = LibraryGame {
            app_id: 123,
            name: "Test Game".to_string(),
            install_path: Some("/tmp/test_game".to_string()),
            is_installed: true,
            playtime_forever_minutes: Some(0),
            active_branch: "public".to_string(),
            update_available: false,
            update_queued: false,
            local_manifest_ids: HashMap::new(),
            is_owned: true,
            is_family_shared: false,
            online_required: None,
        };
        let launch_info = LaunchInfo {
            app_id: 123,
            id: "0".to_string(),
            description: "Test".to_string(),
            executable: "test.exe".to_string(),
            arguments: "".to_string(),
            workingdir: None,
            target: LaunchTarget::WindowsProton,
        };
        let config = crate::config::LauncherConfig::default();

        let result = client.internal_legacy_launch_adhoc(&app, &launch_info, None, &config, None).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Ad-hoc bypass is prohibited"));
    }

    #[tokio::test]
    async fn test_pipeline_integration_scaffolding() {
        // Passing no app causes ResolveGame to fail early.
        let mut ctx = crate::launch::pipeline::PipelineContext::new(999999);
        let pipeline = crate::launch::pipeline::LaunchPipeline::with_default_stages();

        let result = pipeline.run(&mut ctx).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.stage_name, "ResolveGame");
        assert!(err.inner.to_string().contains("App context missing"));
    }

    #[test]
    fn parses_installed_and_disabled_dlc_from_appmanifest() {
        // Base game with two DLC depots installed (tagged with dlcappid) and one of
        // those DLC explicitly disabled.
        let manifest = r#""AppState"
{
	"appid"		"1000"
	"InstalledDepots"
	{
		"1001"
		{
			"manifest"	"123"
			"size"		"456"
			"dlcappid"	"2001"
		}
		"1002"
		{
			"manifest"	"789"
			"size"		"12"
			"dlcappid"	"2002"
		}
	}
	"UserConfig"
	{
		"DisabledDLC"		"2002"
	}
}
"#;

        let installed = parse_installed_dlc_appids(manifest);
        assert!(installed.contains(&2001));
        assert!(installed.contains(&2002));
        assert_eq!(installed.len(), 2);

        let disabled = parse_disabled_dlc_appids(manifest);
        assert_eq!(disabled, HashSet::from([2002]));
    }

    #[test]
    fn parses_comma_separated_disabled_dlc_list() {
        let manifest = r#""AppState"
{
	"MountedConfig"
	{
		"DisabledDLC"		"3001,3002, 3003"
	}
}
"#;
        let disabled = parse_disabled_dlc_appids(manifest);
        assert_eq!(disabled, HashSet::from([3001, 3002, 3003]));
        assert!(parse_installed_dlc_appids(manifest).is_empty());
    }

    #[test]
    fn parses_linux_launch_section_from_vdf() {
        let raw = r#""appinfo"
{
  "appid" "10"
  "config"
  {
    "launch"
    {
      "0"
      {
        "executable" "linux/game.sh"
        "arguments" "-foo -bar"
        "oslist" "linux"
      }
    }
  }
}"#;

        let launch_options = parse_launch_info_from_vdf(10, raw).expect("parse launch info");
        let launch = &launch_options[0];
        assert_eq!(launch.target, LaunchTarget::NativeLinux);
        assert_eq!(launch.executable, "linux/game.sh");
        assert_eq!(launch.arguments, "-foo -bar");
    }

    #[test]
    fn extracts_dlc_ids_from_listofdlc() {
        // Mirrors a real PICS appinfo: sections nested under an appid-keyed root,
        // DLC declared in `extended/listofdlc`. Regression guard for the daemon
        // returning an empty DLC list when appinfo isn't the text-only shape.
        let raw = r#""1794680"
{
  "common" { "name" "Vampire Survivors" }
  "extended" { "listofdlc" "2305610,2305620, 2305630,2305640,2305650" }
}"#;
        let vdf = find_vdf_in_pics(raw.as_bytes()).expect("parse pics vdf");
        let section = pics_app_section(vdf.value());

        assert_eq!(section.get_str(&["common", "name"]), Some("Vampire Survivors"));
        assert_eq!(
            dlc_ids_from_section(section),
            vec![2305610, 2305620, 2305630, 2305640, 2305650],
        );
    }

    #[test]
    fn dlc_ids_empty_when_no_listofdlc() {
        let raw = r#""appinfo" { "common" { "name" "No DLC Game" } }"#;
        let vdf = find_vdf_in_pics(raw.as_bytes()).expect("parse pics vdf");
        let section = pics_app_section(vdf.value());
        assert!(dlc_ids_from_section(section).is_empty());
    }

    #[test]
    fn parses_ufs_savefile_rules() {
        let raw = r#""2784470"
{
  "ufs"
  {
    "savefiles"
    {
      "0"
      {
        "root" "WinAppDataLocalLow"
        "path" "SadSocket/9Kings"
        "pattern" "*"
        "recursive" "1"
      }
      "1"
      {
        "root" "GameInstall"
        "path" "Saves"
        "pattern" "*.sav"
      }
    }
  }
}"#;
        let vdf = find_vdf_in_pics(raw.as_bytes()).expect("parse pics vdf");
        let specs = ufs_save_specs_from_section(pics_app_section(vdf.value()));
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].root, "WinAppDataLocalLow");
        assert_eq!(specs[0].path, "SadSocket/9Kings");
        assert!(specs[0].recursive);
        assert_eq!(specs[1].root, "GameInstall");
        assert_eq!(specs[1].pattern, "*.sav");
        assert!(!specs[1].recursive); // absent recursive defaults to false
    }
}

fn split_args(args: &str) -> Vec<String> {
    args.split_whitespace().map(ToString::to_string).collect()
}


impl SteamClient {
    pub fn find_mangohud_lib() -> Option<PathBuf> {
        // Common install locations across distros
        let candidates = [
            "/usr/lib/mangohud/libMangoHud.so",
            "/usr/lib/mangohud/libMangoHud_dlsym.so",
            "/usr/lib/x86_64-linux-gnu/mangohud/libMangoHud.so",
            "/usr/lib64/mangohud/libMangoHud.so",
            "/usr/local/lib/mangohud/libMangoHud.so",
            "/usr/local/lib/x86_64-linux-gnu/mangohud/libMangoHud.so",
        ];

        for path in candidates {
            let p = PathBuf::from(path);
            if p.exists() {
                return Some(p);
            }
        }

        // Try ldconfig as fallback
        if let Ok(output) = std::process::Command::new("ldconfig")
            .args(["-p"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if line.contains("libMangoHud") {
                    if let Some(path) = line.split("=>").nth(1) {
                        let p = PathBuf::from(path.trim());
                        if p.exists() {
                            return Some(p);
                        }
                    }
                }
            }
        }

        None
    }
}
