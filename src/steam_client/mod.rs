use crate::core::acf::extract_quoted_values;
use crate::core::config::{
    delete_session, library_cache_path, load_launcher_config, load_library_cache, load_session,
    save_library_cache, save_session,
};
use crate::core::models::{
    DepotPlatform, DlcState, DownloadProgress, DownloadProgressState, LibraryGame,
    ManifestSelection, OwnedGame, QrEvent, SessionState, SteamGuardReq,
};
use crate::library::cloud_sync::{
    CloudClient, CloudPathResolver, UfsConfig, UfsRootOverride, UfsSaveSpec, default_cloud_root,
};
use crate::library::depot_browser::ManifestFileEntry;
use crate::web::cm_list::get_cm_endpoints;
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use protobuf::{Message, MessageField};
use steam_vent::auth::{
    AuthConfirmationHandler, ClientInfo, ConfirmationAction, ConfirmationMethod,
    DeviceConfirmationHandler, FileGuardDataStore, RefreshToken,
    UserProvidedAuthConfirmationHandler,
};
use steam_vent::connection::Connection;
use steam_vent::{ConnectionError, ConnectionTrait, LoginError, ServerList};
use steam_vent_proto::steammessages_clientserver::{
    CMsgClientGetAppOwnershipTicket, CMsgClientRequestEncryptedAppTicket,
};
use steam_vent_proto::steammessages_clientserver_2::{
    CMsgClientGetDepotDecryptionKey, CMsgClientGetDepotDecryptionKeyResponse,
};
use steam_vent_proto::steammessages_clientserver_appinfo::{
    CMsgClientPICSAccessTokenRequest, CMsgClientPICSAccessTokenResponse,
    CMsgClientPICSProductInfoRequest, CMsgClientPICSProductInfoResponse,
    cmsg_client_picsproduct_info_request,
};
use steam_vent_proto::steammessages_clientserver_userstats::{
    CMsgClientGetUserStats, CMsgClientGetUserStatsResponse,
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
use steam_vent_proto::steammessages_storebrowse_steamclient::{
    CStoreBrowse_GetItems_Request, CStoreBrowse_GetItems_Response, EStoreAppType,
    StoreBrowseContext, StoreBrowseItemDataRequest, StoreItem, StoreItemID,
};
use tokio::io::{AsyncWriteExt, duplex, sink};
use tokio::sync::mpsc::Receiver;

mod achievements;
pub mod connection;
mod dlc;
mod download;
pub mod licence_list;
pub mod pics;
mod registry;
mod store_mapping;
mod types;
mod ufs;

use achievements::*;
pub use categories::category_online_required;
use connection::*;
use dlc::*;
use download::*;
use pics::*;
#[cfg_attr(not(target_os = "windows"), allow(unused_imports))]
use registry::*;
pub use store_mapping::*;
pub use types::*;
use ufs::*;

#[derive(Clone)]
pub struct SteamClient {
    connection: Option<Connection>,
    state: LoginState,
    connected_at: Option<Instant>,
    active_cm: Option<SocketAddr>,
    server_list: Option<ServerList>,
    pending_confirmations: Vec<ConfirmationPrompt>,
    /// Told which confirmation methods Steam allows during a password login.
    confirmation_hook: Option<ConfirmationHook>,
    /// Why the last session restore failed.
    restore_error: Option<Arc<anyhow::Error>>,
}

// SteamClient methods are implemented across these submodules.
mod anticheat;

mod chat;

mod client;

pub use client::{CodeReader, ConfirmationHook};

mod friends;

mod install;

mod market;

use install::DepotJob;
pub use install::InstallOptions;

mod content;

mod depots;

mod depot_files;

pub(crate) use content::ContentHost;

mod language;

mod launch;

mod library;

mod manage;

pub use launch::{
    LaunchFlags, LaunchTargetChoice, check_launch_option_target, find_launch_option,
    forced_steam_reason, hide_uninstalled_launch_options, launch_executable_installed,
    launch_targets, select_launch_entry,
};

mod cloudconfig;

mod collections;

mod manifests;
pub use manifests::AppManifestSpec;

mod process;

mod storequery;

mod tags;

pub mod tags_table;

mod workshop;

mod workshop_manifest;

pub use storequery::{DealsScope, QueryPage};

mod events;

pub use events::StoreEvent;

mod players;

mod wishlist;

pub use wishlist::WishlistEntry;

pub mod ident;

mod profile;

pub use profile::UserProfile;

mod categories;

pub mod genres_table;

pub use chat::{PlayingSession, check_invite};

pub use collections::RemoteNamespace;

pub use friends::{AddedFriend, Friend, Roster, WatcherEnd, host_steam_owns_presence};

pub use market::{
    InventoryItem, MarketPrice, MarketSearchResult, MyMarketState, WalletBalance, inventory_via,
    market_price, market_search, my_listings_via, wallet_via,
};

/// Terminate the process with `pid` and any children it spawned.
///
/// On Windows the launched process is often a thin launcher that re-spawns the
/// real game, so we kill the whole tree with `taskkill /T`. On Unix we send
/// `SIGTERM` then `SIGKILL` to give the game a chance to exit cleanly first.
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
    let sanitized = sanitized.trim().to_string();
    // Dot-only or empty names escape common/.
    if crate::core::paths::is_safe_dir_name(&sanitized) {
        sanitized
    } else {
        "_".to_string()
    }
}

/// Install folder, never `App N` placeholder.
pub(crate) fn choose_install_dir(
    appid: u32,
    pics_installdir: Option<String>,
    existing_installdir: Option<String>,
) -> Result<String> {
    let is_placeholder = |d: &str| d.trim().eq_ignore_ascii_case(&format!("App {appid}"));
    match (existing_installdir, pics_installdir) {
        (Some(dir), Some(_)) => Ok(dir),
        (Some(dir), None) if !is_placeholder(&dir) => Ok(dir),
        (None, Some(dir)) => Ok(dir),
        _ => bail!(
            "Steam's app info for app {appid} has no install directory, so Aurelia won't guess a \
             folder name. Try again later; if it keeps failing, the app may not be installable."
        ),
    }
}

/// Best-effort: relocate app `appid`'s entry from `from_lib` to `to_lib` in
/// `libraryfolders.vdf` (or just add it when `from_lib == to_lib`, e.g. for an
/// import). Logs and continues on any problem — Steam reconciles the index from
/// the appmanifests on its next launch.
async fn update_libraryfolders_for(from_lib: &Path, to_lib: &Path, appid: u32, install_dir: &Path) {
    let roots = crate::library::all_library_roots().await;
    let Some(vdf_path) = crate::library::relocate::find_libraryfolders_vdf(&roots) else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&vdf_path) else {
        tracing::warn!("could not read libraryfolders.vdf");
        return;
    };
    let size = crate::core::fsutil::dir_size(install_dir);
    match crate::library::relocate::update_libraryfolders_apps(&text, appid, from_lib, to_lib, size)
    {
        Some(updated) => {
            if let Err(e) = crate::core::atomic::write_atomic(&vdf_path, updated.as_bytes()) {
                tracing::warn!("could not write libraryfolders.vdf: {e}");
            }
        }
        None => tracing::warn!(
            "could not locate the library entry in libraryfolders.vdf; Steam will reconcile on next launch"
        ),
    }
}

impl SteamClient {
    /// Borrow the live CM connection.
    pub(crate) fn require_connection(&self) -> Result<&Connection> {
        self.connection.as_ref().ok_or_else(|| {
            crate::core::error::TypedError::new(
                crate::core::error::ErrorKind::AuthRequired,
                "not logged in — run `aurelia login` first",
            )
            .into()
        })
    }

    /// Cloned connection for spawned tasks.
    pub(crate) fn require_connection_owned(&self) -> Result<Connection> {
        self.require_connection().cloned()
    }

    /// [`pics_app_buffer`] against this client's live connection.
    pub(crate) async fn pics_buffer(
        &self,
        appid: u32,
        job_context: &'static str,
    ) -> Result<Vec<u8>> {
        pics_app_buffer(self.require_connection()?, appid, job_context).await
    }

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
        if let Ok(output) = std::process::Command::new("ldconfig").args(["-p"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if line.contains("libMangoHud")
                    && let Some(path) = line.split("=>").nth(1)
                {
                    let p = PathBuf::from(path.trim());
                    if p.exists() {
                        return Some(p);
                    }
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests;
