//! Value enums and their conversions.

use clap::ValueEnum;

use aurelia::core::models::DepotPlatform;

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum PlatformArg {
    Windows,
    Linux,
}

/// `play --steam-mode` choices.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum SteamModeArg {
    /// The in-Wine Steam runtime.
    Runtime,
    /// Bridge to the host Steam client.
    Host,
    /// Standalone, no Steam client.
    None,
}

/// `proton anticheat` choices.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum AntiCheatArg {
    /// Proton EasyAntiCheat Runtime.
    Eac,
    /// Proton BattlEye Runtime.
    Battleye,
}

/// Choices for on/off settings.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum OnOffArg {
    /// Enable the setting.
    On,
    /// Disable the setting.
    Off,
}

/// `config shader-precache` choices.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum ShaderPrecacheArg {
    /// Replay pipelines before launch.
    Auto,
    /// Leave shaders to the game.
    Off,
}

impl From<ShaderPrecacheArg> for aurelia::launch::shaders::ShaderPrecache {
    fn from(value: ShaderPrecacheArg) -> Self {
        match value {
            ShaderPrecacheArg::Auto => Self::Auto,
            ShaderPrecacheArg::Off => Self::Off,
        }
    }
}

/// kbit/s or `off`/`unlimited`.
pub(crate) fn parse_download_limit(raw: &str) -> Result<u64, String> {
    let value = raw.trim();
    if ["off", "none", "unlimited"]
        .iter()
        .any(|w| value.eq_ignore_ascii_case(w))
    {
        return Ok(0);
    }
    value
        .parse::<u64>()
        .map_err(|_| format!("`{raw}` is not a number of kbit/s or `off`"))
}

/// `config install-resume` choices.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum InstallResumeArg {
    /// Wait for `install resume`.
    Manual,
    /// Daemon resumes interrupted installs.
    Auto,
}

impl From<InstallResumeArg> for aurelia::library::backlog::InstallResume {
    fn from(value: InstallResumeArg) -> Self {
        match value {
            InstallResumeArg::Manual => Self::Manual,
            InstallResumeArg::Auto => Self::Auto,
        }
    }
}

/// `--steam-runtime auto|on|off`: per-game policy for the self-contained Windows Steam
/// runtime (background master Steam in Wine).
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum SteamRuntimeArg {
    /// Default behavior (currently off).
    Auto,
    /// Start the master Steam client in Wine for this game's Steamworks/DRM handshake.
    On,
    /// Never use the master Steam runtime for this game.
    Off,
}

impl From<SteamRuntimeArg> for aurelia::core::models::SteamRuntimePolicy {
    fn from(value: SteamRuntimeArg) -> Self {
        use aurelia::core::models::SteamRuntimePolicy;
        match value {
            SteamRuntimeArg::Auto => SteamRuntimePolicy::Auto,
            SteamRuntimeArg::On => SteamRuntimePolicy::Enabled,
            SteamRuntimeArg::Off => SteamRuntimePolicy::Disabled,
        }
    }
}

/// `config game --cloud-sync` choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum CloudSyncArg {
    /// Follow `config cloud-sync`.
    Auto,
    /// Sync around `play`.
    On,
    /// Never sync around `play`.
    Off,
}

/// Per-game `--wayland-native` choices.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum WaylandNativeArg {
    /// Follow `config wayland-native`.
    Auto,
    /// Use Wine/SDL Wayland drivers.
    On,
    /// Always use X11.
    Off,
}

impl WaylandNativeArg {
    /// Per-game override; `None` follows global.
    pub(crate) fn as_override(self) -> Option<bool> {
        match self {
            WaylandNativeArg::Auto => None,
            WaylandNativeArg::On => Some(true),
            WaylandNativeArg::Off => Some(false),
        }
    }
}

impl CloudSyncArg {
    /// Per-game override; `None` follows global.
    pub(crate) fn as_override(self) -> Option<bool> {
        match self {
            CloudSyncArg::Auto => None,
            CloudSyncArg::On => Some(true),
            CloudSyncArg::Off => Some(false),
        }
    }
}

/// `--steam-prefix-mode shared|per-game`: how the master Steam prefix backs a game.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum SteamPrefixModeArg {
    /// Run the game in the master Steam prefix directly.
    Shared,
    /// Copy/symlink Steam into the game's own prefix.
    PerGame,
}

impl From<SteamPrefixModeArg> for aurelia::core::models::SteamPrefixMode {
    fn from(value: SteamPrefixModeArg) -> Self {
        use aurelia::core::models::SteamPrefixMode;
        match value {
            SteamPrefixModeArg::Shared => SteamPrefixMode::Shared,
            SteamPrefixModeArg::PerGame => SteamPrefixMode::PerGame,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum ChatPresenceArg {
    Online,
    Offline,
}

impl From<ChatPresenceArg> for aurelia::core::config::ChatPresence {
    fn from(value: ChatPresenceArg) -> Self {
        match value {
            ChatPresenceArg::Online => aurelia::core::config::ChatPresence::Online,
            ChatPresenceArg::Offline => aurelia::core::config::ChatPresence::Offline,
        }
    }
}

/// Which side wins when `cloud sync` finds a diverged save.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum CloudResolve {
    /// Overwrite the local copy with the cloud copy.
    Cloud,
    /// Overwrite the cloud copy with the local copy.
    Local,
}

impl From<CloudResolve> for aurelia::library::cloud_sync::ConflictPolicy {
    fn from(value: CloudResolve) -> Self {
        match value {
            CloudResolve::Cloud => aurelia::library::cloud_sync::ConflictPolicy::TakeCloud,
            CloudResolve::Local => aurelia::library::cloud_sync::ConflictPolicy::TakeLocal,
        }
    }
}

/// Sort order for `workshop browse`, mapped to an `EPublishedFileQueryType`.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum WorkshopSort {
    /// Trending now (default).
    Trend,
    /// Highest rated overall.
    Popular,
    /// Most recently published.
    Recent,
    /// Most recently updated.
    Updated,
    /// Most subscribed.
    Subscriptions,
    /// Best text-search relevance (use with `--search`).
    Text,
}

impl WorkshopSort {
    pub(crate) fn query_type(self) -> u32 {
        match self {
            WorkshopSort::Trend => 3,
            WorkshopSort::Popular => 0,
            WorkshopSort::Recent => 1,
            WorkshopSort::Updated => 21,
            WorkshopSort::Subscriptions => 9,
            WorkshopSort::Text => 12,
        }
    }
}

/// Thumbs-up/down for `workshop rate`.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum VoteArg {
    Up,
    Down,
}

impl From<PlatformArg> for DepotPlatform {
    fn from(value: PlatformArg) -> Self {
        match value {
            PlatformArg::Windows => DepotPlatform::Windows,
            PlatformArg::Linux => DepotPlatform::Linux,
        }
    }
}
