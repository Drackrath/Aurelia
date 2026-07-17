use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum SteamPrefixMode {
    #[default]
    Shared, // use master_steam_prefix WINEPREFIX directly
    PerGame, // copy/symlink Steam into game's own compatdata prefix
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum SteamRuntimePolicy {
    #[default]
    Auto,
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteamLaunchConfig {
    #[serde(default = "default_true")]
    pub no_browser: bool, // kill CEF/steamwebhelper entirely
    #[serde(default = "default_true")]
    pub no_friends_ui: bool, // no friends list window
    #[serde(default = "default_true")]
    pub no_overlay: bool, // no in-game overlay
    #[serde(default = "default_true")]
    pub no_chat_ui: bool, // no chat popups
    #[serde(default)]
    pub no_vr: bool, // no OpenVR/SteamVR
    #[serde(default)]
    pub big_picture: bool, // force Big Picture (lighter than desktop UI)
}

pub fn default_true() -> bool {
    true
}

impl Default for SteamLaunchConfig {
    fn default() -> Self {
        Self {
            no_browser: true,
            no_friends_ui: true,
            no_overlay: true,
            no_chat_ui: true,
            no_vr: false,
            big_picture: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum GraphicsBackendPolicy {
    #[default]
    Auto,
    WineD3D,
    DXVK,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum D3D12ProviderPolicy {
    #[default]
    Auto,
    Vkd3dProton,
    Vkd3dWine,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphicsLayerConfig {
    #[serde(default)]
    pub dxvk_enabled: bool,
    #[serde(default)]
    pub vkd3d_proton_enabled: bool,
    #[serde(default)]
    pub vkd3d_enabled: bool,
    #[serde(default = "default_true")]
    pub nvapi_enabled: bool,
    #[serde(default)]
    pub graphics_backend_policy: GraphicsBackendPolicy,
    #[serde(default)]
    pub d3d12_policy: D3D12ProviderPolicy,
    #[serde(default)]
    pub use_symlinks_in_prefix: bool,
    #[serde(default)]
    pub custom_dxvk_path: Option<PathBuf>,
    #[serde(default)]
    pub custom_vkd3d_path: Option<PathBuf>,
    #[serde(default)]
    pub custom_vkd3d_proton_path: Option<PathBuf>,
}

impl Default for GraphicsLayerConfig {
    fn default() -> Self {
        Self {
            dxvk_enabled: false,
            vkd3d_proton_enabled: false,
            vkd3d_enabled: false,
            nvapi_enabled: true,
            graphics_backend_policy: GraphicsBackendPolicy::Auto,
            d3d12_policy: D3D12ProviderPolicy::Auto,
            use_symlinks_in_prefix: false,
            custom_dxvk_path: None,
            custom_vkd3d_path: None,
            custom_vkd3d_proton_path: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserAppConfig {
    pub launch_options: String,                 // e.g. "-novid -console"
    pub env_variables: HashMap<String, String>, // e.g. {"MANGOHUD": "1"}
    pub use_steam_runtime: bool,                // DEPRECATED: use steam_runtime_policy instead
    #[serde(default)]
    pub steam_runtime_policy: SteamRuntimePolicy,
    #[serde(default)]
    pub steam_prefix_mode: SteamPrefixMode,
    #[serde(default)]
    pub steam_launch_config: SteamLaunchConfig,
    #[serde(default)]
    pub graphics_layers: GraphicsLayerConfig,
    #[serde(default)]
    pub gpu_preference: Option<String>,
    pub hidden: bool,   // Future use
    pub favorite: bool, // Future use
}

pub type UserConfigStore = HashMap<u32, UserAppConfig>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionState {
    pub account_name: Option<String>,
    pub steam_id: Option<u64>,
    pub refresh_token: Option<String>,
    pub client_instance_id: Option<u64>,
    /// Web-scoped access token pasted from the browser (`login --web-token`).
    /// Enables the web-surface commands (inventory/wallet/listings) without a
    /// CM session; short-lived (expiry is inside the JWT). Not a refresh token.
    #[serde(default)]
    pub web_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedGame {
    pub app_id: u32,
    pub name: String,
    pub playtime_forever_minutes: u32,
    #[serde(default)]
    pub local_manifest_ids: HashMap<u64, u64>,
    #[serde(default)]
    pub update_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub steam_id: u64,
    pub account_name: String,
    pub game_count: usize,
    pub is_online: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalGame {
    pub app_id: u32,
    pub name: String,
    pub install_dir: PathBuf,
    pub proton_version: Option<String>,
    #[serde(default = "default_branch")]
    pub active_branch: String,
}

fn default_branch() -> String {
    "public".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameModel {
    pub app_id: u32,
    pub name: String,
    pub playtime_forever_minutes: Option<u32>,
    pub install_dir: Option<PathBuf>,
    pub proton_version: Option<String>,
    pub image_cache_path: Option<PathBuf>,
}

impl GameModel {
    pub fn installed(&self) -> bool {
        self.install_dir.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryGame {
    pub app_id: u32,
    pub name: String,
    pub playtime_forever_minutes: Option<u32>,
    pub is_installed: bool,
    pub install_path: Option<String>,
    #[serde(default)]
    pub local_manifest_ids: HashMap<u64, u64>,
    #[serde(default)]
    pub update_available: bool,
    #[serde(default)]
    pub update_queued: bool,
    #[serde(default = "default_branch")]
    pub active_branch: String,
    /// Whether the logged-in account holds a license for this game (it appears in
    /// the account's owned-games list). `false` means the game is accessible only
    /// via Family Sharing. Defaults to `true` for backwards compatibility.
    #[serde(default = "default_true")]
    pub is_owned: bool,
    /// Whether the game is installed locally but licensed to a different account
    /// (i.e. borrowed through Steam Family Sharing).
    #[serde(default)]
    pub is_family_shared: bool,
    /// Whether the game appears to require an online connection to play, inferred
    /// from its PICS store categories (online multiplayer with no single-player
    /// support). `None` means it hasn't been determined yet — this is only
    /// populated on demand (e.g. `aurelia list --online`) because it requires a
    /// per-app PICS fetch. See [`crate::steam_client::category_online_required`].
    #[serde(default)]
    pub online_required: Option<bool>,
    /// Platform whose depot is installed locally (`"windows"`, `"linux"` or
    /// `"macos"`), determined from the files on disk. `None` when the game isn't
    /// installed or the platform couldn't be determined. Lets a driver (e.g.
    /// Heroic) tell a native-Linux game (run directly) from a Windows game (run
    /// through Proton) without re-deriving it.
    #[serde(default)]
    pub platform: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GameLibrary {
    pub games: Vec<LibraryGame>,
}

/// Per-DLC status for a base game, combining account ownership with the local
/// install/enable state recorded in the base game's appmanifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlcState {
    pub app_id: u32,
    /// The account holds a license for this DLC (an app ownership ticket was issued).
    pub owned: bool,
    /// The DLC's content is present on disk (its depots are recorded in the base
    /// game's appmanifest, tagged with this DLC's appid).
    pub installed: bool,
    /// The DLC is listed in the base game's `DisabledDLC`, so Steam treats it as off.
    pub disabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryFilter {
    All,
    Installed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ExecutableArchitecture {
    #[default]
    Unknown,
    X86,
    X86_64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DepotPlatform {
    Linux,
    Windows,
}

pub struct ManifestSelection {
    pub app_id: u32,
    pub depot_id: u32,
    pub manifest_id: u64,
    pub appinfo_vdf: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SteamGuardReq {
    EmailCode { domain_hint: String },
    DeviceCode,
    DeviceConfirmation,
}

/// A progress event from the QR login flow, handed to the caller's callback.
#[derive(Debug, Clone, Copy)]
pub enum QrEvent<'a> {
    /// A challenge URL to render as a QR code — emitted initially and again each
    /// time Steam rotates or regenerates the code.
    Challenge(&'a str),
    /// The code was scanned in the Steam Mobile app; Steam is now awaiting the
    /// user's approval. Emitted once per session.
    Scanned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DownloadProgressState {
    #[default]
    Queued,
    Downloading,
    Verifying,
    /// Relocating an installed game's files between library folders.
    Moving,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DownloadProgress {
    pub state: DownloadProgressState,
    /// Bytes downloaded across the whole app (all depots).
    pub bytes_downloaded: u64,
    /// Total bytes for the whole app (all selected depots).
    pub total_bytes: u64,
    pub current_file: String,
    /// The depot currently being downloaded (0 when not applicable).
    pub depot_id: u32,
    /// Bytes downloaded for the current depot only.
    pub depot_bytes_downloaded: u64,
    /// Total bytes for the current depot only.
    pub depot_total_bytes: u64,
}

#[derive(Clone, Default)]
pub struct DownloadState {
    pub is_downloading: bool,
    pub is_paused: bool,
    pub app_id: u32,
    pub app_name: String,
    /// Total bytes for the whole app (all selected depots).
    pub total_bytes: u64,
    /// Bytes downloaded across the whole app (all depots).
    pub downloaded_bytes: u64,
    pub status_text: String,
    /// Depot currently being downloaded.
    pub depot_id: u32,
    /// Total bytes for the current depot.
    pub depot_total_bytes: u64,
    /// Bytes downloaded for the current depot.
    pub depot_downloaded_bytes: u64,
    pub abort_signal: Arc<AtomicBool>,
}

#[derive(Debug, serde::Deserialize)]
pub struct AppInfoRoot {
    #[serde(default)]
    pub appinfo: Option<AppInfoNode>,
    #[serde(default)]
    pub common: Option<CommonNode>,
    #[serde(default)]
    pub depots: HashMap<String, DepotNode>,
    #[serde(default)]
    pub branches: HashMap<String, BranchNode>,
    #[serde(default)]
    pub config: Option<ConfigNode>,
    #[serde(default)]
    pub extended: Option<ExtendedNode>,
}

#[derive(Debug, serde::Deserialize)]
pub struct AppInfoNode {
    #[serde(default)]
    pub common: Option<CommonNode>,
    #[serde(default)]
    pub depots: HashMap<String, DepotNode>,
    #[serde(default)]
    pub branches: HashMap<String, BranchNode>,
    #[serde(default)]
    pub config: Option<ConfigNode>,
    #[serde(default)]
    pub extended: Option<ExtendedNode>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ExtendedNode {
    /// For DLC, the base game's appid this content depends on.
    #[serde(default)]
    pub dependantonapp: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ConfigNode {
    #[serde(default)]
    pub launch: HashMap<String, ProductLaunchEntry>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ProductLaunchEntry {
    #[serde(default)]
    pub executable: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub oslist: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct CommonNode {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub dlc: HashMap<String, String>,
    #[serde(default)]
    pub installdir: Option<String>,
    /// Application type, e.g. "Game", "DLC", "Demo".
    #[serde(rename = "type", default)]
    pub app_type: Option<String>,
    /// For some DLC, the base game's appid.
    #[serde(default)]
    pub parent: Option<String>,
    /// Store category flags, keyed `category_<id>` with value `"1"`. Steam has no
    /// dedicated "online required" field, so we infer it from these (online
    /// multiplayer categories combined with the absence of single-player). See
    /// [`crate::steam_client::category_online_required`].
    #[serde(default)]
    pub category: HashMap<String, String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct DepotNode {
    #[serde(default)]
    pub config: Option<DepotConfig>,
    #[serde(default)]
    pub manifests: Option<DepotManifests>,
    #[serde(flatten)]
    pub _other: HashMap<String, serde_json::Value>,
}

#[derive(Debug, serde::Deserialize)]
pub struct BranchNode {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub pwdrequired: Option<String>,
    #[serde(default)]
    pub buildid: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct DepotConfig {
    #[serde(default)]
    pub oslist: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct DepotManifests {
    #[serde(default)]
    pub public: Option<String>,
}

/// Distinguishes a standalone Workshop item from a collection (a Workshop entry
/// whose "content" is a list of member item ids rather than downloadable files).
#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq)]
pub enum WorkshopItemKind {
    Item,
    Collection,
}

/// Metadata for a single Steam Workshop published file, distilled from
/// `PublishedFile.GetDetails`. For SteamPipe-backed items `hcontent_file` is the
/// manifest gid used to download content; legacy UGC items instead expose a
/// `file_url`. Collections carry their member ids in `children` and have no
/// content of their own.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkshopItem {
    /// `publishedfileid`.
    pub id: u64,
    /// `consumer_appid` — the game this item belongs to.
    pub app_id: u32,
    /// The creator's SteamID64 (`creator`). Needed to address the item's comment
    /// thread; `0` if Steam didn't report it.
    pub creator: u64,
    pub title: String,
    /// SteamPipe manifest gid; `0` for legacy/collection entries.
    pub hcontent_file: u64,
    /// Legacy UGC download URL; empty when the item is SteamPipe-backed.
    pub file_url: String,
    pub file_size: u64,
    pub time_updated: i64,
    pub kind: WorkshopItemKind,
    /// Collection member ids (empty for plain items).
    pub children: Vec<u64>,
}

/// A Workshop item recorded as installed in `appworkshop_<appid>.acf` (local
/// on-disk state, independent of the live Steam metadata in [`WorkshopItem`]).
#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkshopInstalledInfo {
    pub id: u64,
    /// The installed content manifest gid (`hcontent_file`). Comparing this to the
    /// item's current `hcontent_file` tells you whether an update is available.
    pub manifest_id: u64,
    pub size: u64,
    pub time_updated: i64,
}

/// A single comment on a Workshop item's comment thread.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkshopComment {
    /// `gidcomment` — the comment's unique id.
    pub id: u64,
    /// The author's SteamID64.
    pub author: u64,
    pub timestamp: i64,
    pub text: String,
    pub upvotes: i32,
}

/// One page of `PublishedFile.QueryFiles` results (browse/search). `next_cursor`
/// is fed back as the `cursor` for the following page; an empty/repeated cursor
/// means there are no more results.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkshopQueryPage {
    pub items: Vec<WorkshopItem>,
    /// Total matching items across all pages (as reported by Steam).
    pub total: u32,
    pub next_cursor: String,
}
