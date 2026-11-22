//! Public DTOs returned by the client.

use super::*;

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
    /// `common/metacritic_score`.
    pub metacritic_score: Option<i64>,
    /// `common/metacritic_fullurl`.
    pub metacritic_url: Option<String>,
    /// `extended/homepage`.
    pub homepage: Option<String>,
    /// `common/genres` values (store genre ids).
    pub genre_ids: Vec<u32>,
    /// `common/category` keys (`category_N`).
    pub category_ids: Vec<u32>,
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

/// One depot's current manifest on one branch, as advertised by PICS appinfo.
/// Steam only exposes the *current* id per branch — historical (older) ids are
/// not enumerable here and must be looked up on SteamDB. Emitted by
/// [`SteamClient::list_depot_manifests`] (the `aurelia manifests` command).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DepotManifestInfo {
    pub depot_id: u32,
    pub depot_name: Option<String>,
    pub branch: String,
    pub manifest_id: u64,
    pub size: u64,
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

/// What `install --dry-run` reports.
#[derive(Debug, Clone, Default)]
pub struct InstallPreview {
    pub estimate: InstallSizeEstimate,
    /// Valid `--language` values, sorted.
    pub languages: Vec<String>,
    /// What a plain install picks.
    pub default_language: Option<String>,
    /// Installed manifest's `UserConfig` language.
    pub installed_language: Option<String>,
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
    /// Public persona
    pub persona_name: Option<String>,
    pub country: String,      // GeoIP Country
    pub authed_machines: u32, // Steam Guard count
    pub flags: u32,           // Account Flags
    pub email: String,
    pub email_validated: bool,
    pub vac_bans: u32, // Num VAC bans
    pub vac_banned_apps: Vec<u32>,
}

/// Encrypted app ticket request outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncryptedTicketOutcome {
    /// Serialized `EncryptedAppTicket` protobuf message.
    Issued(Vec<u8>),
    /// Steam declined; carries the eresult.
    Refused(i32),
}

/// A game available to the account through Steam Family Sharing.
#[derive(Debug, Clone)]
pub struct SharedApp {
    pub app_id: u32,
    pub name: String,
    /// SteamID64 of the family member who owns (lends) this app, if reported.
    pub owner_steamid: Option<u64>,
}
