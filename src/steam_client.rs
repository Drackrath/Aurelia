use crate::cloud_sync::{default_cloud_root, CloudClient};
use crate::cm_list::get_cm_endpoints;
use crate::config::{
    delete_session, library_cache_path, load_launcher_config, load_library_cache, load_session,
    save_library_cache, save_session,
};
use crate::depot_browser::{self, DepotInfo as BrowserDepotInfo, ManifestFileEntry};
use crate::models::{
    AppInfoRoot, DepotPlatform, DlcState, DownloadProgress, DownloadProgressState, LibraryGame,
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
#[derive(Debug, Clone, Default)]
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
#[derive(Debug, Clone, Default)]
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

impl SteamClient {
    pub fn new() -> Result<Self> {
        Ok(Self {
            connection: None,
            state: LoginState::Connected,
            connected_at: None,
            active_cm: None,
            server_list: None,
            pending_confirmations: Vec::new(),
        })
    }

    pub fn is_authenticated(&self) -> bool {
        self.connection.is_some()
    }

    pub fn is_offline(&self) -> bool {
        self.state == LoginState::Offline
    }

    pub fn connection(&self) -> Option<&Connection> {
        self.connection.as_ref()
    }

    /// Build a Steam Cloud client over the current connection (for save sync).
    pub fn cloud_client(&self) -> Result<crate::cloud_sync::CloudClient> {
        let connection = self
            .connection
            .as_ref()
            .cloned()
            .context("steam connection not initialized")?;
        Ok(crate::cloud_sync::CloudClient::new(connection))
    }

    /// SteamID64 of the logged-in account, if connected.
    pub fn steam_id(&self) -> Option<u64> {
        self.connection
            .as_ref()
            .map(|connection| u64::from(connection.steam_id()))
    }

    pub async fn logout(&mut self) -> Result<()> {
        self.connection = None;
        self.state = LoginState::Connected;
        delete_session().await?;
        Ok(())
    }

    pub async fn get_app_ticket(&self, appid: u32) -> Result<Vec<u8>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut request = CMsgClientGetAppOwnershipTicket::new();
        request.set_app_id(appid);

        let response: steam_vent_proto::steammessages_clientserver::CMsgClientGetAppOwnershipTicketResponse =
            connection
                .job(request)
                .await
                .context("failed requesting app ownership ticket")?;

        let ticket = response.ticket().to_vec();
        if ticket.is_empty() {
            bail!("Steam returned an empty app ownership ticket for app {appid}");
        }
        Ok(ticket)
    }

    pub async fn get_account_data(&self) -> AccountData {
        let Some(connection) = self.connection.as_ref() else {
            return AccountData::default();
        };

        let mut data = AccountData {
            steam_id: u64::from(connection.steam_id()),
            country: connection.ip_country_code().unwrap_or_default(),
            ..Default::default()
        };

        // Attempt to populate from persistent session info
        if let Ok(session) = load_session().await {
            if let Some(name) = session.account_name {
                data.account_name = name;
            }
        }

        if data.account_name.is_empty() {
            data.account_name = "Steam User".to_string();
        }

        data.email = "Hidden".to_string();
        data.email_validated = true;

        data
    }

    pub fn pending_confirmations(&self) -> &[ConfirmationPrompt] {
        &self.pending_confirmations
    }

    pub fn clear_pending_confirmations(&mut self) {
        self.pending_confirmations.clear();
    }

    pub fn is_auth_error_text(message: &str) -> bool {
        let msg = message.to_ascii_lowercase();
        msg.contains("invalid access token")
            || msg.contains("not logged on")
            || msg.contains("apierror(notloggedon)")
            || msg.contains("expired")
            || msg.contains("session")
    }

    pub async fn connect(&mut self) -> Result<()> {
        tracing::debug!("Connecting to Steam: resolving CM server list ...");
        match self.resolve_server_list().await {
            Ok(server_list) => {
                self.active_cm = Some(server_list.pick());
                self.connected_at = Some(Instant::now());
                self.state = LoginState::Connected;
                Ok(())
            }
            Err(err) => {
                if self.try_enter_offline_mode().await? {
                    tracing::warn!("Steam unavailable; entering offline mode");
                    return Ok(());
                }
                Err(err)
            }
        }
    }

    async fn resolve_server_list(&mut self) -> Result<ServerList> {
        if let Some(existing) = &self.server_list {
            return Ok(existing.clone());
        }

        tracing::debug!("Discovering Steam CM servers ...");
        match ServerList::discover().await {
            Ok(list) => {
                tracing::debug!("Discovered Steam CM server list");
                self.server_list = Some(list.clone());
                Ok(list)
            }
            Err(_) => {
                tracing::debug!("CM discovery failed; falling back to bootstrap endpoints");
                let tcp_servers = get_cm_endpoints().await;
                if tcp_servers.is_empty() {
                    bail!("failed to discover Steam CM servers and no fallback endpoints were available")
                }

                let ws_servers = tcp_servers
                    .iter()
                    .map(|entry| format!("{}:{}", entry.ip(), entry.port()))
                    .collect();

                let list = ServerList::new(tcp_servers, ws_servers)
                    .context("failed constructing fallback server list")?;
                self.server_list = Some(list.clone());
                Ok(list)
            }
        }
    }

    async fn try_enter_offline_mode(&mut self) -> Result<bool> {
        let cache_path = library_cache_path()?;
        if cache_path.exists() {
            self.state = LoginState::Offline;
            self.connection = None;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn invalidate_session(&mut self) {
        self.connection = None;
        self.state = LoginState::Connected;
    }

    pub fn connected_seconds(&self) -> Option<u64> {
        self.connected_at.map(|v| v.elapsed().as_secs())
    }

    pub fn active_cm(&self) -> Option<SocketAddr> {
        self.active_cm
    }

    pub async fn restore_session(&mut self) -> Result<SessionState> {
        let persisted = load_session().await?;
        let account_name = persisted
            .account_name
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow!("no persisted account_name found"))?;
        let refresh_token = persisted
            .refresh_token
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow!("no persisted refresh_token found"))?;

        self.connect().await?;
        if self.is_offline() {
            bail!("offline mode: using cached library");
        }
        self.state = LoginState::AwaitingAccessTokenLogon;

        let server_list = self.resolve_server_list().await?;
        let connection = access_with_retry(&server_list, &account_name, &refresh_token).await?;

        self.connection = Some(connection);
        let session = self
            .session_from_connection(account_name)
            .context("refresh token login succeeded but no token was available for persistence")?;
        save_session(&session).await?;
        self.state = LoginState::Complete;
        self.pending_confirmations.clear();
        Ok(session)
    }

    /// Log in with account credentials.
    ///
    /// * `guard_code` — a Steam Guard code supplied up front (non-interactive).
    /// * `interactive_pin` — when `true` and no `guard_code` is given, Steam Guard
    ///   codes are read **interactively from stdin** (the `login --pin` flow). The
    ///   prompt appears only once Steam has begun the auth session, so email codes
    ///   (which Steam sends at that point) work correctly.
    pub async fn login(
        &mut self,
        account_name: String,
        password: String,
        guard_code: Option<String>,
        interactive_pin: bool,
    ) -> Result<SessionState> {
        self.connect().await?;
        if self.is_offline() {
            bail!("offline mode: using cached library");
        }

        self.state = LoginState::AwaitingCredentialSession;
        let server_list = self.resolve_server_list().await?;

        self.state = LoginState::AwaitingGuardConfirmation;
        self.state = LoginState::AwaitingPollResult;
        self.state = LoginState::AwaitingAccessTokenLogon;

        let login_result = if let Some(code) = guard_code.filter(|v| !v.trim().is_empty()) {
            let (mut writer, reader) = duplex(64);
            writer
                .write_all(format!("{}\n", code.trim()).as_bytes())
                .await
                .context("failed to prepare guard code input")?;
            drop(writer);

            let handler = UserProvidedAuthConfirmationHandler::new(reader, sink())
                .or(DeviceConfirmationHandler);

            Connection::login(
                &server_list,
                &account_name,
                &password,
                FileGuardDataStore::user_cache(),
                handler,
            )
            .await
        } else if interactive_pin {
            // Read the Steam Guard code from stdin when Steam asks for it; fall back
            // to mobile-app approval if the account only allows that.
            tracing::info!(
                "Login method awaited: Steam Guard code — enter it when prompted below"
            );
            let handler =
                UserProvidedAuthConfirmationHandler::new(tokio::io::stdin(), tokio::io::stderr())
                    .or(DeviceConfirmationHandler);

            Connection::login(
                &server_list,
                &account_name,
                &password,
                FileGuardDataStore::user_cache(),
                handler,
            )
            .await
        } else {
            Connection::login(
                &server_list,
                &account_name,
                &password,
                FileGuardDataStore::user_cache(),
                DeviceConfirmationHandler,
            )
            .await
        };

        let connection = match login_result {
            Ok(connection) => connection,
            Err(ConnectionError::UnsupportedConfirmationAction(methods)) => {
                self.pending_confirmations =
                    methods.iter().map(map_confirmation).collect::<Vec<_>>();
                bail!("Steam Guard confirmation required")
            }
            Err(other) => return Err(anyhow!(other)).context("steam-vent login flow failed"),
        };

        self.connection = Some(connection);
        let session = self
            .session_from_connection(account_name)
            .context("login succeeded but no token was available for persistence")?;
        save_session(&session).await?;
        self.state = LoginState::Complete;
        self.pending_confirmations.clear();
        Ok(session)
    }

    /// Log in by having the user scan a QR code with the Steam Mobile app.
    ///
    /// Drives Steam's `Authentication.BeginAuthSessionViaQR` / `PollAuthSessionStatus`
    /// flow over an anonymous connection (these are unauthenticated service methods).
    /// `on_challenge` is invoked with the challenge URL to display — initially and
    /// again whenever Steam rotates the code — so the caller can render the QR.
    pub async fn login_qr<F>(&mut self, mut on_challenge: F) -> Result<SessionState>
    where
        F: FnMut(&str),
    {
        use steam_vent_proto::steammessages_auth_steamclient::{
            CAuthentication_BeginAuthSessionViaQR_Request,
            CAuthentication_BeginAuthSessionViaQR_Response,
            CAuthentication_PollAuthSessionStatus_Request,
            CAuthentication_PollAuthSessionStatus_Response, EAuthTokenPlatformType,
        };

        self.connect().await?;
        if self.is_offline() {
            bail!("offline mode: cannot start QR login");
        }
        let server_list = self.resolve_server_list().await?;

        // The QR begin/poll calls don't require a logged-in session, so route them
        // over an anonymous connection.
        let anon = Connection::anonymous(&server_list)
            .await
            .map_err(|e| anyhow!(e))
            .context("failed to open anonymous Steam connection for QR login")?;

        let mut begin = CAuthentication_BeginAuthSessionViaQR_Request::new();
        begin.set_device_friendly_name("Aurelia CLI".to_string());
        begin.set_platform_type(EAuthTokenPlatformType::k_EAuthTokenPlatformType_SteamClient);
        begin.set_website_id("Client".to_string());

        let begin_resp: CAuthentication_BeginAuthSessionViaQR_Response = anon
            .service_method(begin)
            .await
            .map_err(|e| anyhow!(e))
            .context("Authentication.BeginAuthSessionViaQR failed")?;

        let client_id = begin_resp.client_id();
        let request_id = begin_resp.request_id().to_vec();
        // Steam suggests a poll interval; clamp to a sane floor.
        let poll_interval = Duration::from_secs_f32(begin_resp.interval().max(2.0));
        let mut challenge_url = begin_resp.challenge_url().to_string();

        on_challenge(&challenge_url);
        tracing::info!("Login method awaited: QR code — scan it with the Steam Mobile app");

        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if Instant::now() >= deadline {
                bail!("QR login timed out after 3 minutes without approval");
            }
            tokio::time::sleep(poll_interval).await;

            let mut poll = CAuthentication_PollAuthSessionStatus_Request::new();
            poll.set_client_id(client_id);
            poll.set_request_id(request_id.clone());

            let resp: CAuthentication_PollAuthSessionStatus_Response =
                match tokio::time::timeout(CM_CONNECT_TIMEOUT, anon.service_method(poll)).await {
                    Ok(Ok(resp)) => resp,
                    Ok(Err(e)) => {
                        return Err(anyhow!(e))
                            .context("Authentication.PollAuthSessionStatus failed");
                    }
                    Err(_) => {
                        tracing::warn!("QR status poll timed out; retrying ...");
                        continue;
                    }
                };

            // Steam periodically rotates the QR; re-render only on an actual change.
            if resp.has_new_challenge_url() && resp.new_challenge_url() != challenge_url {
                challenge_url = resp.new_challenge_url().to_string();
                on_challenge(&challenge_url);
            }

            let refresh_token = resp.refresh_token();
            if !refresh_token.is_empty() {
                let account_name = resp.account_name().to_string();
                tracing::info!(account = %account_name, "QR login approved");

                // Exchange the issued refresh token for a live connection.
                let connection =
                    access_with_retry(&server_list, &account_name, refresh_token).await?;
                let steam_id = Some(u64::from(connection.steam_id()));
                self.connection = Some(connection);

                let session = SessionState {
                    account_name: Some(account_name),
                    steam_id,
                    refresh_token: Some(refresh_token.to_string()),
                    client_instance_id: None,
                };
                save_session(&session).await?;
                self.state = LoginState::Complete;
                self.pending_confirmations.clear();
                return Ok(session);
            }
        }
    }

    fn session_from_connection(&self, account_name: String) -> Option<SessionState> {
        let connection = self.connection.as_ref()?;
        let steam_id = u64::from(connection.steam_id());
        Some(SessionState {
            account_name: Some(account_name),
            steam_id: Some(steam_id),
            refresh_token: connection.access_token().map(ToString::to_string),
            client_instance_id: None,
        })
    }

    pub async fn fetch_branches(&self, appid: u32) -> Result<Vec<String>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut request = CMsgClientPICSProductInfoRequest::new();
        request
            .apps
            .push(cmsg_client_picsproduct_info_request::AppInfo {
                appid: Some(appid),
                ..Default::default()
            });

        let response: CMsgClientPICSProductInfoResponse = connection
            .job(request)
            .await
            .context("failed requesting appinfo product info for branches")?;

        let app = response
            .apps
            .iter()
            .find(|entry| entry.appid() == appid)
            .ok_or_else(|| anyhow!("missing app info payload for app {appid}"))?;

        let appinfo_vdf = String::from_utf8_lossy(app.buffer()).to_string();
        let parsed: AppInfoRoot =
            parse_appinfo(&appinfo_vdf).context("failed parsing appinfo VDF")?;

        let branches = parsed
            .appinfo
            .map(|node| node.branches)
            .unwrap_or(parsed.branches);

        let mut names: Vec<String> = branches
            .into_iter()
            .filter(|(_, node)| node.pwdrequired.is_none()) // Ignore private
            .map(|(name, _)| name)
            .collect();

        if !names.contains(&"public".to_string()) {
            names.push("public".to_string());
        }

        names.sort();
        Ok(names)
    }

    pub async fn get_available_platforms(
        &mut self,
        appid: u32,
    ) -> Result<(Vec<DepotPlatform>, Vec<u8>)> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut request = CMsgClientPICSProductInfoRequest::new();
        request
            .apps
            .push(cmsg_client_picsproduct_info_request::AppInfo {
                appid: Some(appid),
                ..Default::default()
            });

        let response: CMsgClientPICSProductInfoResponse = connection
            .job(request)
            .await
            .context("failed requesting appinfo product info")?;

        let app = response
            .apps
            .iter()
            .find(|entry| entry.appid() == appid)
            .ok_or_else(|| anyhow!("missing app info payload for app {appid}"))?;

        let buffer = app.buffer().to_vec();
        let appinfo_vdf_text = String::from_utf8_lossy(&buffer);

        let mut has_linux = false;
        let mut has_windows = false;

        let vdf_res = steam_vdf_parser::parse_binary(&buffer)
            .or_else(|_| steam_vdf_parser::parse_text(&appinfo_vdf_text).map(|v| v.into_owned()));

        if let Ok(vdf) = vdf_res {
            let root_obj = vdf.as_obj().unwrap();
            let depots_val = if vdf.key() == "appinfo" || vdf.key() == appid.to_string() {
                root_obj.get("depots")
            } else {
                root_obj.get("depots").or_else(|| {
                    root_obj
                        .values()
                        .next()
                        .and_then(|v| v.as_obj())
                        .and_then(|o| o.get("depots"))
                })
            };

            if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
                for value in depots.values() {
                    let oslist = value
                        .get_obj(&["config"])
                        .and_then(|c| c.get("oslist"))
                        .and_then(|o| o.as_str());

                    if let Some(os) = oslist {
                        let os = os.to_lowercase();
                        if os.contains("linux") {
                            has_linux = true;
                        }
                        if os.contains("windows") {
                            has_windows = true;
                        }
                    }
                }
            }
        } else {
            tracing::warn!("get_available_platforms: VDF parse failed for {appid}, using fallback discovery");
            return Ok((vec![DepotPlatform::Windows, DepotPlatform::Linux], buffer));
        }

        let mut platforms = Vec::new();
        if has_windows {
            platforms.push(DepotPlatform::Windows);
        }
        if has_linux {
            platforms.push(DepotPlatform::Linux);
        }

        if platforms.is_empty() {
            platforms.push(DepotPlatform::Windows);
        }

        Ok((platforms, buffer))
    }

    pub async fn install_game(
        &self,
        appid: u32,
        platform: DepotPlatform,
        cached_vdf: Option<Vec<u8>>,
        filter_depots: Option<Vec<u64>>,
        shared_state: Arc<std::sync::RwLock<crate::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        let connection = self
            .connection
            .as_ref()
            .cloned()
            .context("steam connection not initialized")?;

        let cfg = load_launcher_config().await?;
        let library_root = cfg.steam_library_path.clone();
        let (game_name, pics_installdir) = self.resolve_install_game_info(appid).await;
        let installdir = pics_installdir.unwrap_or_else(|| sanitize_install_dir(&game_name));

        // If this app is a DLC, its content must land in the base game's install
        // directory and be registered in the base game's appmanifest (so the game
        // sees the DLC as installed/enabled) rather than getting its own manifest.
        let dlc_parent = self.resolve_dlc_parent(appid).await;
        let dlc_appid = dlc_parent.map(|_| appid);

        let (install_dir, manifest_path) = if let Some(base_appid) = dlc_parent {
            let base_manifest = self.appmanifest_path(base_appid).await?;
            if !base_manifest.exists() {
                bail!(
                    "cannot install DLC {appid}: its base game (app {base_appid}) is not installed — install it first"
                );
            }
            let base_raw = std::fs::read_to_string(&base_manifest)
                .with_context(|| format!("failed reading {}", base_manifest.display()))?;
            let base_installdir = parse_installdir_from_acf(&base_raw).ok_or_else(|| {
                anyhow!("could not determine base game install dir for app {base_appid}")
            })?;
            let steamapps = base_manifest
                .parent()
                .ok_or_else(|| anyhow!("invalid base manifest path for app {base_appid}"))?;
            let dir = steamapps.join("common").join(&base_installdir);
            tracing::info!(
                "DLC {appid} -> installing into base game {base_appid} at {}",
                dir.display()
            );
            (dir, base_manifest)
        } else {
            let dir = Path::new(&library_root)
                .join("steamapps")
                .join("common")
                .join(&installdir);
            let mp = Path::new(&library_root)
                .join("steamapps")
                .join(format!("appmanifest_{appid}.acf"));
            (dir, mp)
        };

        std::fs::create_dir_all(&install_dir)
            .with_context(|| format!("failed creating {}", install_dir.display()))?;

        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let client_clone = self.clone();
        let shared_state_clone = shared_state.clone();

        tokio::task::spawn(async move {
            let _ = tx
                .send(DownloadProgress {
                    state: DownloadProgressState::Queued,
                    current_file: String::new(),
                    ..Default::default()
                })
                .await;


            let appinfo_vdf_bytes_owned;
            let appinfo_vdf_bytes = if let Some(cached) = cached_vdf {
                appinfo_vdf_bytes_owned = cached;
                &appinfo_vdf_bytes_owned
            } else {
                let mut request = CMsgClientPICSProductInfoRequest::new();
                request
                    .apps
                    .push(cmsg_client_picsproduct_info_request::AppInfo {
                        appid: Some(appid),
                        ..Default::default()
                    });

                let response: CMsgClientPICSProductInfoResponse = match connection.job(request).await
                {
                    Ok(res) => res,
                    Err(e) => {
                        let _ = tx
                            .send(DownloadProgress {
                                state: DownloadProgressState::Failed,
                                current_file: format!("failed requesting appinfo: {e}"),
                                ..Default::default()
                            })
                            .await;
                        return;
                    }
                };

                let app = response.apps.iter().find(|entry| entry.appid() == appid);
                let Some(app) = app else {
                    let _ = tx
                        .send(DownloadProgress {
                            state: DownloadProgressState::Failed,
                            current_file: "missing appinfo payload".to_string(),
                            ..Default::default()
                        })
                        .await;
                    return;
                };
                appinfo_vdf_bytes_owned = app.buffer().to_vec();
                &appinfo_vdf_bytes_owned
            };

            let appinfo_vdf_text = String::from_utf8_lossy(appinfo_vdf_bytes).to_string();


            let mut selections = Vec::new();
            // Build id of the installed content (from PICS), recorded in the appmanifest
            // so the Steam launcher sees the install as current and doesn't re-download.
            let mut build_id: Option<String> = None;
            // Sum of all selected depots' max (uncompressed) sizes — the whole-app total
            // used to report overall download progress across depots.
            let mut grand_total_bytes: u64 = 0;

            let mut has_windows = false;
            if let Ok(map) = parse_pics_product_info(appinfo_vdf_bytes) {
                // To keep filtering, we re-parse or re-use the find_vdf logic.
                // We'll re-parse here to stay strictly compliant with Task 2's request to call parse_pics_product_info.
                if let Ok(vdf) = find_vdf_in_pics(appinfo_vdf_bytes) {
                    let root_obj = vdf.as_obj().unwrap();
                    let depots_val = if vdf.key() == "appinfo" || vdf.key() == appid.to_string() {
                        root_obj.get("depots")
                    } else {
                        root_obj.get("depots").or_else(|| {
                            root_obj
                                .get("appinfo")
                                .and_then(|v| v.as_obj())
                                .and_then(|o| o.get("depots"))
                        })
                    };

                    // depots -> branches -> public -> buildid
                    build_id = depots_val
                        .and_then(|d| d.get_obj(&["branches", "public"]))
                        .and_then(|b| b.get("buildid"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
                        for (key, value) in depots.iter() {
                            if let Ok(d_id) = key.parse::<u32>() {
                                let oslist = value
                                    .get_obj(&["config"])
                                    .and_then(|c| c.get("oslist"))
                                    .and_then(|o| o.as_str());

                                if oslist
                                    .map(|os| os.to_lowercase().contains("windows"))
                                    .unwrap_or(false)
                                {
                                    has_windows = true;
                                }

                                let mut match_os = should_keep_depot(oslist, platform);

                                if match_os {
                                    // 1. LANGUAGE CHECK
                                    let lang = value
                                        .get_obj(&["config"])
                                        .and_then(|c| c.get("language"))
                                        .and_then(|l| l.as_str());
                                    if let Some(lang) = lang {
                                        if lang != "english" && !lang.is_empty() {
                                            match_os = false;
                                        }
                                    }
                                }

                                if match_os {
                                    let depot_id_u64 = d_id as u64;
                                    let is_allowed = match &filter_depots {
                                        Some(list) => list.contains(&depot_id_u64),
                                        None => true,
                                    };

                                    if is_allowed {
                                        if let Some(m_id) = map.get(&depot_id_u64) {
                                            // Uncompressed size for this depot. Prefer the
                                            // per-manifest size (present even when the
                                            // depot-level "maxsize" is absent/zero).
                                            grand_total_bytes += value
                                                .get_obj(&["manifests", "public"])
                                                .and_then(|m| m.get("size"))
                                                .and_then(|v| v.as_str())
                                                .and_then(|s| s.parse::<u64>().ok())
                                                .or_else(|| {
                                                    value
                                                        .get("maxsize")
                                                        .and_then(|v| v.as_str())
                                                        .and_then(|s| s.parse::<u64>().ok())
                                                })
                                                .unwrap_or(0);
                                            selections.push(ManifestSelection {
                                                app_id: appid,
                                                depot_id: d_id,
                                                manifest_id: *m_id,
                                                appinfo_vdf: appinfo_vdf_text.clone(),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                println!("CRITICAL: VDF parse failed for {appid}");
            }

            if selections.is_empty() {

                let msg = if has_windows && matches!(platform, DepotPlatform::Linux) {
                    "No native Linux depots found. This game may only support Windows (Proton)."
                } else {
                    "No matching depots found for the selected platform."
                };

                let _ = tx
                    .send(DownloadProgress {
                        state: DownloadProgressState::Failed,
                        current_file: msg.to_string(),
                        ..Default::default()
                    })
                    .await;
                return;
            }

            let _ = tx
                .send(DownloadProgress {
                    state: DownloadProgressState::Downloading,
                    total_bytes: grand_total_bytes,
                    current_file: format!("starting download of {} depots", selections.len()),
                    ..Default::default()
                })
                .await;

            // Update shared state for the start of the download
            if let Ok(mut state) = shared_state_clone.write() {
                state.is_downloading = true;
                state.is_paused = false;
                state.app_id = appid;
                state.app_name = game_name.clone();
                state.downloaded_bytes = 0;
                // Whole-app total (all selected depots), so progress is reported against
                // the full install size rather than just the current depot.
                state.total_bytes = grand_total_bytes;
                state.status_text = format!("Initializing download for {}...", game_name);
            }

            // Register the install start with Steam: write an "update required"
            // appmanifest up front so the launcher sees the app as installing rather
            // than missing. (Skipped for DLC, whose content lives in the base game's
            // manifest — overwriting that here would mark the base game for re-download.)
            if dlc_appid.is_none() {
                if let Err(e) = SteamClient::write_appmanifest(
                    &manifest_path,
                    appid,
                    &game_name,
                    &installdir,
                    Vec::new(),
                    build_id.as_deref(),
                    false,
                ) {
                    tracing::warn!("failed writing initial appmanifest for app {appid}: {e}");
                } else {
                    tracing::info!(
                        "Registered install start with Steam for app {appid} (buildid {})",
                        build_id.as_deref().unwrap_or("0")
                    );
                }
            }

            // Periodically forward the live byte counters over the channel. The
            // download callbacks only mutate the shared state; this reporter is what
            // turns that into the progress the CLI renders.
            let progress_tx = tx.clone();
            let progress_state = shared_state_clone.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_millis(250));
                loop {
                    ticker.tick().await;
                    let snapshot = match progress_state.read() {
                        Ok(s) => Some((
                            s.is_downloading,
                            s.downloaded_bytes,
                            s.total_bytes,
                            s.status_text.clone(),
                            s.depot_id,
                            s.depot_downloaded_bytes,
                            s.depot_total_bytes,
                        )),
                        Err(_) => None,
                    };
                    let Some((
                        downloading,
                        downloaded,
                        total,
                        status,
                        depot_id,
                        depot_downloaded,
                        depot_total,
                    )) = snapshot
                    else {
                        break;
                    };
                    if !downloading {
                        break;
                    }
                    // Stop if the receiver is gone (terminal message already consumed).
                    if progress_tx
                        .send(DownloadProgress {
                            state: DownloadProgressState::Downloading,
                            bytes_downloaded: downloaded,
                            total_bytes: total,
                            current_file: status,
                            depot_id,
                            depot_bytes_downloaded: depot_downloaded,
                            depot_total_bytes: depot_total,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });

            // 2. Fetch Content Servers via Service
            tracing::info!("Fetching Content Servers for AppID: {}...", appid);
            let hosts = match client_clone.get_content_servers(connection.cell_id()).await {
                Ok(h) => h,
                Err(e) => {
                    let _ = tx
                        .send(DownloadProgress {
                            state: DownloadProgressState::Failed,
                            current_file: format!("Failed to fetch content servers: {}", e),
                            ..Default::default()
                        })
                        .await;
                    return;
                }
            };

            // 3. Download Loop
            let mut success = true;
            let mut successful_depots = Vec::new();
            for selection in selections {
                tracing::info!(
                    "Starting download for Depot {} (GID: {})...",
                    selection.depot_id,
                    selection.manifest_id
                );
                if let Ok(mut state) = shared_state_clone.write() {
                    state.status_text = format!("Downloading depot {}", selection.depot_id);
                    // Reset the current-depot counters so per-depot progress restarts.
                    state.depot_id = selection.depot_id;
                    state.depot_downloaded_bytes = 0;
                    state.depot_total_bytes = 0;
                }

                let key = match client_clone.get_depot_key(appid, selection.depot_id).await {
                    Ok(k) => k,
                    Err(e) => {
                        tracing::warn!(
                            "Skipping Depot {} (No Key/Not Owned): {}",
                            selection.depot_id,
                            e
                        );
                        continue;
                    }
                };
                // A valid depot key is exactly 32 bytes; a short/all-zero key would
                // decrypt chunks to garbage (the chunk path then fails the zip parse
                // with "Could not find EOCD").
                tracing::debug!(
                    "Depot {} key: {} bytes, all_zero={}",
                    selection.depot_id,
                    key.len(),
                    key.iter().all(|&b| b == 0)
                );

                let manifest_code = match client_clone
                    .get_manifest_request_code(appid, selection.depot_id, selection.manifest_id)
                    .await
                {
                    Ok(code) => Some(code),
                    Err(e) => {
                        tracing::warn!(
                            "Failed to get manifest request code for depot {}: {}",
                            selection.depot_id,
                            e
                        );
                        None
                    }
                };

                let mut depot_success = false;
                for host in &hosts {
                    let token = match client_clone
                        .get_cdn_auth_token(appid, selection.depot_id, host)
                        .await
                    {
                        Ok(t) => Some(t),
                        Err(e) => {
                            tracing::warn!("Failed to get auth token for host {}: {}", host, e);
                            None
                        }
                    };

                    let (host_name, port) = if let Some(pos) = host.find(':') {
                        (
                            &host[..pos],
                            host[pos + 1..].parse::<u16>().unwrap_or(80),
                        )
                    } else {
                        (host.as_str(), 80)
                    };

                    let cdn_server = steam_cdn::web_api::content_service::CDNServer {
                        r#type: "CDN".to_string(),
                        https: port == 443,
                        host: host_name.to_string(),
                        vhost: host_name.to_string(),
                        port,
                        cell_id: connection.cell_id(),
                        load: 0,
                        weighted_load: 0,
                        auth_token: token,
                    };

                    let cdn_client = steam_cdn::CDNClient::with_server(
                        Arc::new(connection.clone()),
                        cdn_server,
                    );

                let state_for_closure = shared_state_clone.clone();
                let on_progress = Arc::new(move |bytes: u64| {
                    if let Ok(mut state) = state_for_closure.write() {
                        // Overall (whole app) and current-depot counters.
                        state.downloaded_bytes += bytes;
                        state.depot_downloaded_bytes += bytes;
                    }
                });

                let state_for_manifest = shared_state_clone.clone();
                let depot_size = Arc::new(std::sync::atomic::AtomicU64::new(0));
                let size_clone = depot_size.clone();
                let grand_total_fallback = grand_total_bytes;
                let on_manifest = Arc::new(move |total_bytes: u64| {
                    size_clone.store(total_bytes, std::sync::atomic::Ordering::SeqCst);
                    if let Ok(mut state) = state_for_manifest.write() {
                        // The manifest gives this depot's exact uncompressed size.
                        state.depot_total_bytes = total_bytes;
                        // If PICS carried no maxsize for the whole app, fall back to
                        // accumulating per-depot totals so overall progress still has a
                        // denominator.
                        if grand_total_fallback == 0 {
                            state.total_bytes += total_bytes;
                        }
                    }
                });

                let abort_signal = shared_state_clone
                    .read()
                    .ok()
                    .map(|s| s.abort_signal.clone());

                    match cdn_client
                        .download_depot(
                            appid,
                            selection.depot_id,
                            selection.manifest_id,
                            &key,
                            &install_dir,
                            manifest_code,
                            false, // verify_mode: false
                            abort_signal,
                            Some(on_progress),
                            Some(on_manifest.clone()),
                        )
                        .await
                    {
                        Ok(_) => {
                            let aborted = shared_state_clone.read()
                                .map(|s| s.abort_signal.load(std::sync::atomic::Ordering::Relaxed))
                                .unwrap_or(false);
                            if aborted {
                                break;
                            }

                            tracing::info!(
                                "Depot {} download complete from {}!",
                                selection.depot_id,
                                host
                            );
                            depot_success = true;
                            successful_depots.push((
                                selection.depot_id,
                                selection.manifest_id,
                                depot_size.load(std::sync::atomic::Ordering::SeqCst),
                            ));
                            break;
                        }
                        Err(e) => {
                            tracing::error!("CDN Error from {}: {}", host, e);
                        }
                    }
                }

                if !depot_success {
                    let aborted = shared_state_clone.read()
                        .map(|s| s.abort_signal.load(std::sync::atomic::Ordering::Relaxed))
                        .unwrap_or(false);

                    if aborted {
                        success = false;
                        break;
                    }

                    let _ = tx
                        .send(DownloadProgress {
                            state: DownloadProgressState::Failed,
                            current_file: format!(
                                "Failed to download depot {} from all available servers",
                                selection.depot_id
                            ),
                            ..Default::default()
                        })
                        .await;
                    success = false;
                    break;
                }
            }

            if success {
                if let Ok(mut state) = shared_state_clone.write() {
                    state.is_downloading = false;
                    state.status_text = "Download complete".to_string();
                }

                let manifest_result = if let Some(dlc) = dlc_appid {
                    // Register the DLC's depots into the base game's manifest (enable it).
                    SteamClient::enable_dlc_in_appmanifest(&manifest_path, dlc, &successful_depots)
                } else {
                    SteamClient::write_appmanifest(
                        &manifest_path,
                        appid,
                        &game_name,
                        &installdir,
                        successful_depots,
                        build_id.as_deref(),
                        true,
                    )
                };
                if let Err(err) = manifest_result {
                    tracing::warn!("failed updating appmanifest for {}: {}", appid, err);
                } else if dlc_appid.is_none() {
                    tracing::info!(
                        "Wrote appmanifest for app {appid}: fully installed, buildid {}",
                        build_id.as_deref().unwrap_or("0")
                    );
                }
                let _ = tx
                    .send(DownloadProgress {
                        state: DownloadProgressState::Completed,
                        bytes_downloaded: 1,
                        total_bytes: 1,
                        current_file: "completed".to_string(),
                        ..Default::default()
                    })
                    .await;
            } else {
                if let Ok(mut state) = shared_state_clone.write() {
                    state.is_downloading = false;
                    state.status_text = "Download failed".to_string();
                }
            }
        });

        Ok(rx)
    }

    pub async fn update_app_branch(&self, appid: u32, branch: &str) -> Result<()> {
        let manifest_path = self.appmanifest_path(appid).await?;
        if !manifest_path.exists() {
            bail!("appmanifest not found for app {appid}");
        }

        let raw = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed reading {}", manifest_path.display()))?;

        let rewritten = rewrite_app_branch(&raw, branch);
        std::fs::write(&manifest_path, rewritten)
            .with_context(|| format!("failed writing {}", manifest_path.display()))?;

        Ok(())
    }

    pub async fn uninstall_game(&self, appid: u32, delete_prefix: bool) -> Result<()> {
        let cfg = load_launcher_config().await?;
        let steamapps = PathBuf::from(cfg.steam_library_path).join("steamapps");
        let appmanifest = steamapps.join(format!("appmanifest_{appid}.acf"));

        let install_dir = if appmanifest.exists() {
            let raw = std::fs::read_to_string(&appmanifest)
                .with_context(|| format!("failed reading {}", appmanifest.display()))?;
            parse_installdir_from_acf(&raw)
                .map(|dir| steamapps.join("common").join(dir))
                .unwrap_or_else(|| steamapps.join("common").join(appid.to_string()))
        } else {
            steamapps.join("common").join(appid.to_string())
        };

        if install_dir.exists() {
            std::fs::remove_dir_all(&install_dir)
                .with_context(|| format!("failed deleting {}", install_dir.display()))?;
        }

        if appmanifest.exists() {
            std::fs::remove_file(&appmanifest)
                .with_context(|| format!("failed deleting {}", appmanifest.display()))?;
        }

        if delete_prefix {
            let compat = steamapps.join("compatdata").join(appid.to_string());
            if compat.exists() {
                std::fs::remove_dir_all(&compat)
                    .with_context(|| format!("failed deleting {}", compat.display()))?;
            }
        }

        Ok(())
    }

    /// Move an installed game to a different Steam library folder.
    ///
    /// Relocates the game files (`steamapps/common/<installdir>`), the Proton
    /// prefix (`steamapps/compatdata/<appid>`, if present) and the
    /// `appmanifest_<appid>.acf`, then updates `libraryfolders.vdf`'s `apps` index
    /// so the Steam client recognises the game at its new path instead of
    /// reporting it as missing. Returns a progress stream (`Moving` events).
    ///
    /// Steam should not be running during the move — it overwrites these files on
    /// exit. The source is only deleted after a successful copy, so an interrupted
    /// move never loses the original install.
    pub async fn move_install(
        &self,
        appid: u32,
        dest_library: PathBuf,
    ) -> Result<Receiver<DownloadProgress>> {
        use crate::relocate;

        // --- Resolve the source layout from the appmanifest ---
        let src_manifest = self.appmanifest_path(appid).await?;
        if !src_manifest.exists() {
            bail!("app {appid} is not installed (no appmanifest found)");
        }
        let src_steamapps = src_manifest
            .parent()
            .ok_or_else(|| anyhow!("invalid manifest path for app {appid}"))?
            .to_path_buf();
        let src_lib_root = src_steamapps
            .parent()
            .ok_or_else(|| anyhow!("invalid library path for app {appid}"))?
            .to_path_buf();

        let raw = std::fs::read_to_string(&src_manifest)
            .with_context(|| format!("failed reading {}", src_manifest.display()))?;
        let installdir = parse_installdir_from_acf(&raw)
            .ok_or_else(|| anyhow!("appmanifest for {appid} has no installdir"))?;

        let src_common = src_steamapps.join("common").join(&installdir);
        if !src_common.exists() {
            bail!("install directory not found: {}", src_common.display());
        }

        // --- Resolve and validate the destination library ---
        let dest_steamapps = dest_library.join("steamapps");
        if !dest_steamapps.exists() {
            bail!(
                "{} is not a Steam library folder (no steamapps/). Add the drive in \
                 Steam \u{2192} Settings \u{2192} Storage first.",
                dest_library.display()
            );
        }
        if dest_steamapps == src_steamapps {
            bail!("app {appid} is already in {}", dest_library.display());
        }

        let dest_common = dest_steamapps.join("common").join(&installdir);
        if dest_common.exists() {
            bail!("destination already exists: {}", dest_common.display());
        }
        let dest_manifest = dest_steamapps.join(format!("appmanifest_{appid}.acf"));

        // Proton prefix, if this game has one.
        let src_compat = src_steamapps.join("compatdata").join(appid.to_string());
        let src_compat = src_compat.exists().then_some(src_compat);
        let dest_compat = src_compat
            .as_ref()
            .map(|_| dest_steamapps.join("compatdata").join(appid.to_string()));

        // Locate the (single) libraryfolders.vdf and warn if the destination isn't
        // a registered library — Steam won't scan an unregistered folder.
        let roots = crate::library::all_library_roots().await;
        if !roots.iter().any(|r| r.join("steamapps") == dest_steamapps) {
            tracing::warn!(
                "{} is not a registered Steam library; Steam may not show the game until \
                 the folder is added in Settings \u{2192} Storage",
                dest_library.display()
            );
        }
        let libraryfolders = relocate::find_libraryfolders_vdf(&roots);

        let (tx, rx) = tokio::sync::mpsc::channel(64);

        tokio::task::spawn_blocking(move || {
            let result = (|| -> Result<()> {
                let _ = tx.blocking_send(DownloadProgress {
                    state: DownloadProgressState::Queued,
                    current_file: "sizing".to_string(),
                    ..Default::default()
                });

                let common_bytes = relocate::dir_size(&src_common);
                let compat_bytes = src_compat.as_ref().map(|p| relocate::dir_size(p)).unwrap_or(0);
                let total = common_bytes + compat_bytes;

                // Game files.
                relocate::move_dir_with_progress(&src_common, &dest_common, common_bytes, |copied, file| {
                    let _ = tx.blocking_send(DownloadProgress {
                        state: DownloadProgressState::Moving,
                        bytes_downloaded: copied,
                        total_bytes: total,
                        current_file: file.to_string(),
                        ..Default::default()
                    });
                })
                .with_context(|| format!("failed moving game files to {}", dest_common.display()))?;

                // Proton prefix.
                if let (Some(sc), Some(dc)) = (&src_compat, &dest_compat) {
                    relocate::move_dir_with_progress(sc, dc, compat_bytes, |copied, file| {
                        let _ = tx.blocking_send(DownloadProgress {
                            state: DownloadProgressState::Moving,
                            bytes_downloaded: common_bytes + copied,
                            total_bytes: total,
                            current_file: file.to_string(),
                            ..Default::default()
                        });
                    })
                    .with_context(|| format!("failed moving Proton prefix to {}", dc.display()))?;
                }

                // appmanifest: copy to the new library, then remove the original so
                // Steam sees the game in exactly one place.
                std::fs::copy(&src_manifest, &dest_manifest)
                    .with_context(|| format!("failed writing {}", dest_manifest.display()))?;
                std::fs::remove_file(&src_manifest)
                    .with_context(|| format!("failed removing {}", src_manifest.display()))?;

                // libraryfolders.vdf apps index (best-effort; Steam reconciles from
                // the appmanifests on next launch if this can't be edited cleanly).
                if let Some(vdf_path) = &libraryfolders {
                    match std::fs::read_to_string(vdf_path) {
                        Ok(text) => {
                            match relocate::update_libraryfolders_apps(
                                &text, appid, &src_lib_root, &dest_library, common_bytes,
                            ) {
                                Some(updated) => {
                                    if let Err(e) = std::fs::write(vdf_path, updated) {
                                        tracing::warn!(
                                            "moved game but could not write libraryfolders.vdf: {e}"
                                        );
                                    }
                                }
                                None => tracing::warn!(
                                    "could not locate library entries in libraryfolders.vdf; \
                                     Steam will reconcile the index on next launch"
                                ),
                            }
                        }
                        Err(e) => tracing::warn!("could not read libraryfolders.vdf: {e}"),
                    }
                }

                Ok(())
            })();

            match result {
                Ok(()) => {
                    let _ = tx.blocking_send(DownloadProgress {
                        state: DownloadProgressState::Completed,
                        ..Default::default()
                    });
                }
                Err(e) => {
                    let _ = tx.blocking_send(DownloadProgress {
                        state: DownloadProgressState::Failed,
                        current_file: format!("{e:#}"),
                        ..Default::default()
                    });
                }
            }
        });

        Ok(rx)
    }

    /// Whether a game is installed *and* its files are present on disk. Returns
    /// `(available, install_path)` — mirrors Heroic's `isGameAvailable`.
    pub async fn is_game_available(&self, appid: u32) -> (bool, Option<String>) {
        let Ok(manifest) = self.appmanifest_path(appid).await else {
            return (false, None);
        };
        if !manifest.exists() {
            return (false, None);
        }
        match self.install_root_for_app(appid).await {
            Ok(path) => {
                let exists = path.exists();
                (exists, Some(path.to_string_lossy().to_string()))
            }
            Err(_) => (false, None),
        }
    }

    /// Relink an install to a different Steam library **without copying files** —
    /// the game's files must already be present at the destination (e.g. the user
    /// moved them by hand). Relocates the `appmanifest` and updates
    /// `libraryfolders.vdf`; the game files and Proton prefix are never touched.
    /// Steam should not be running. Returns the destination install directory.
    pub async fn relink_install(&self, appid: u32, dest_library: PathBuf) -> Result<PathBuf> {
        let src_manifest = self.appmanifest_path(appid).await?;
        if !src_manifest.exists() {
            bail!("app {appid} is not registered (no appmanifest to relink)");
        }
        let src_steamapps = src_manifest
            .parent()
            .ok_or_else(|| anyhow!("invalid manifest path for app {appid}"))?
            .to_path_buf();
        let src_lib_root = src_steamapps
            .parent()
            .ok_or_else(|| anyhow!("invalid library path for app {appid}"))?
            .to_path_buf();

        let raw = std::fs::read_to_string(&src_manifest)
            .with_context(|| format!("failed reading {}", src_manifest.display()))?;
        let installdir = parse_installdir_from_acf(&raw)
            .ok_or_else(|| anyhow!("appmanifest for {appid} has no installdir"))?;

        let dest_steamapps = dest_library.join("steamapps");
        if !dest_steamapps.exists() {
            bail!(
                "{} is not a Steam library folder (no steamapps/)",
                dest_library.display()
            );
        }
        if dest_steamapps == src_steamapps {
            bail!("app {appid} is already linked to {}", dest_library.display());
        }
        let dest_common = dest_steamapps.join("common").join(&installdir);
        if !dest_common.exists() {
            bail!(
                "game files not found at {} — relink only updates Steam's records; use \
                 `move` to copy the files there first",
                dest_common.display()
            );
        }
        let dest_manifest = dest_steamapps.join(format!("appmanifest_{appid}.acf"));

        // Move only the manifest (files are already in place).
        std::fs::copy(&src_manifest, &dest_manifest)
            .with_context(|| format!("failed writing {}", dest_manifest.display()))?;
        std::fs::remove_file(&src_manifest)
            .with_context(|| format!("failed removing {}", src_manifest.display()))?;

        update_libraryfolders_for(&src_lib_root, &dest_library, appid, &dest_common).await;
        Ok(dest_common)
    }

    /// Import an on-disk install that Steam doesn't know about: write an
    /// `appmanifest_<appid>.acf` for the existing files in `library` and register
    /// it in `libraryfolders.vdf`. Depot manifests and the build id come from PICS
    /// so Steam sees the game as installed and up to date. Steam should not be
    /// running. Returns the install directory.
    pub async fn import_install(
        &self,
        appid: u32,
        library: PathBuf,
        platform: DepotPlatform,
    ) -> Result<PathBuf> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut request = CMsgClientPICSProductInfoRequest::new();
        request
            .apps
            .push(cmsg_client_picsproduct_info_request::AppInfo {
                appid: Some(appid),
                ..Default::default()
            });
        let response: CMsgClientPICSProductInfoResponse = connection
            .job(request)
            .await
            .context("failed requesting appinfo product info for import")?;
        let app = response
            .apps
            .iter()
            .find(|e| e.appid() == appid)
            .ok_or_else(|| anyhow!("missing appinfo payload for app {appid}"))?;

        let vdf = find_vdf_in_pics(app.buffer()).context("failed to parse product info VDF")?;
        let root_obj = vdf.as_obj().context("root is not an object")?;
        let app_obj = if vdf.key() == "appinfo" || vdf.key() == appid.to_string() {
            root_obj
        } else {
            root_obj
                .get("appinfo")
                .and_then(|v| v.as_obj())
                .unwrap_or(root_obj)
        };

        let common = app_obj.get("common").and_then(|v| v.as_obj());
        let name = common
            .and_then(|c| c.get("name"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("App {appid}"));
        let installdir = common
            .and_then(|c| c.get("installdir"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("PICS appinfo for {appid} has no installdir; cannot import"))?;

        let depots_obj = app_obj.get("depots").and_then(|v| v.as_obj());
        let buildid = depots_obj
            .and_then(|d| d.get("branches"))
            .and_then(|v| v.as_obj())
            .and_then(|b| b.get("public"))
            .and_then(|v| v.as_obj())
            .and_then(|p| p.get("buildid"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Platform-matched, non-DLC depots with a public manifest → InstalledDepots.
        let mut installed_depots: Vec<(u32, u64, u64)> = Vec::new();
        if let Some(depots) = depots_obj {
            for (key, value) in depots.iter() {
                let Ok(depot_id) = key.parse::<u32>() else {
                    continue;
                };
                let Some(obj) = value.as_obj() else { continue };
                if obj.get("dlcappid").is_some() {
                    continue;
                }
                let oslist = obj
                    .get("config")
                    .and_then(|v| v.as_obj())
                    .and_then(|c| c.get("oslist"))
                    .and_then(|v| v.as_str());
                if !should_keep_depot(oslist, platform) {
                    continue;
                }
                let Some(public) = obj
                    .get("manifests")
                    .and_then(|v| v.as_obj())
                    .and_then(|m| m.get("public"))
                    .and_then(|v| v.as_obj())
                else {
                    continue;
                };
                if let Some(mid) = public
                    .get("gid")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    let size = public
                        .get("size")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0);
                    installed_depots.push((depot_id, mid, size));
                }
            }
        }

        let steamapps = library.join("steamapps");
        if !steamapps.exists() {
            bail!(
                "{} is not a Steam library folder (no steamapps/)",
                library.display()
            );
        }
        let common_dir = steamapps.join("common").join(&installdir);
        if !common_dir.exists() {
            bail!(
                "game files not found at {} — `import` registers existing files; use \
                 `install` to download the game",
                common_dir.display()
            );
        }
        let manifest = steamapps.join(format!("appmanifest_{appid}.acf"));
        if manifest.exists() {
            bail!("app {appid} is already registered at {}", manifest.display());
        }

        Self::write_appmanifest(
            &manifest,
            appid,
            &name,
            &installdir,
            installed_depots,
            buildid.as_deref(),
            true,
        )?;

        // Register in libraryfolders.vdf (add to this library; nothing to remove).
        update_libraryfolders_for(&library, &library, appid, &common_dir).await;
        Ok(common_dir)
    }

    pub async fn get_content_servers(&self, cell_id: u32) -> Result<Vec<String>> {
        let connection = self.connection.as_ref().ok_or_else(|| anyhow!("No connection"))?;
        let mut request = CContentServerDirectory_GetServersForSteamPipe_Request::new();
        request.set_cell_id(cell_id);
        request.set_max_servers(20);

        let response: CContentServerDirectory_GetServersForSteamPipe_Response = connection
            .service_method(request)
            .await
            .context("failed calling ContentServerDirectory.GetServersForSteamPipe")?;

        let mut hosts = Vec::new();
        for server in &response.servers {
            if server.type_() == "SteamCache" || server.type_() == "CDN" {
                let host = server.host().to_string();
                hosts.push(host);
            }
        }

        if hosts.is_empty() {
            println!("ERROR: Service returned 0 valid CDN servers!");
        }

        Ok(hosts)
    }

    pub async fn get_manifest_request_code(
        &self,
        app_id: u32,
        depot_id: u32,
        manifest_id: u64,
    ) -> Result<u64> {
        let connection = self.connection.as_ref().ok_or_else(|| anyhow!("No connection"))?;
        let mut request = CContentServerDirectory_GetManifestRequestCode_Request::new();
        request.set_app_id(app_id);
        request.set_depot_id(depot_id);
        request.set_manifest_id(manifest_id);

        let response: CContentServerDirectory_GetManifestRequestCode_Response = connection
            .service_method(request)
            .await
            .context("failed calling ContentServerDirectory.GetManifestRequestCode")?;

        let code = response.manifest_request_code();
        // A 0 code means the service-method response came back empty/default — worth
        // surfacing because the subsequent CDN manifest fetch will then fail.
        if code == 0 {
            tracing::warn!(
                "GetManifestRequestCode returned 0 (empty response) for app {app_id} depot {depot_id} manifest {manifest_id}"
            );
        } else {
            tracing::debug!(
                "GetManifestRequestCode for app {app_id} depot {depot_id} manifest {manifest_id} = {code}"
            );
        }
        Ok(code)
    }

    pub async fn get_cdn_auth_token(
        &self,
        app_id: u32,
        depot_id: u32,
        host_name: &str,
    ) -> Result<String> {
        let connection = self.connection.as_ref().ok_or_else(|| anyhow!("No connection"))?;
        let mut request = CContentServerDirectory_GetCDNAuthToken_Request::new();
        request.set_app_id(app_id);
        request.set_depot_id(depot_id);
        request.set_host_name(host_name.to_string());

        let response: CContentServerDirectory_GetCDNAuthToken_Response = connection
            .service_method(request)
            .await
            .context("failed calling ContentServerDirectory.GetCDNAuthToken")?;

        if response.token().is_empty() {
            // An empty token with the expiration field still set is a normal Steam
            // response (many SteamPipe CDNs don't require a per-host token), so this is
            // only a debug-level note, not an anomaly.
            tracing::debug!(
                "GetCDNAuthToken returned an empty token for app {app_id} depot {depot_id} host {host_name} (has_expiration={})",
                response.has_expiration_time()
            );
            return Err(anyhow!("Empty Auth Token returned"));
        }

        tracing::debug!(
            "GetCDNAuthToken for app {app_id} depot {depot_id} host {host_name}: token len {}",
            response.token().len()
        );
        Ok(response.token().to_string())
    }

    pub async fn get_depot_list(&self, app_id: u32) -> Result<Vec<DepotInfo>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut request = CMsgClientPICSProductInfoRequest::new();
        request
            .apps
            .push(cmsg_client_picsproduct_info_request::AppInfo {
                appid: Some(app_id),
                ..Default::default()
            });

        let response: CMsgClientPICSProductInfoResponse = connection
            .job(request)
            .await
            .context("failed requesting appinfo product info for depot list")?;

        let app = response
            .apps
            .iter()
            .find(|entry| entry.appid() == app_id)
            .ok_or_else(|| anyhow!("missing appinfo payload for app {app_id}"))?;

        let mut out = Vec::new();
        if let Ok(vdf) = find_vdf_in_pics(app.buffer()) {
            let root_obj = vdf.as_obj().context("root is not an object")?;
            let depots_val = if vdf.key() == "appinfo" || vdf.key() == app_id.to_string() {
                root_obj.get("depots")
            } else {
                root_obj.get("depots").or_else(|| {
                    root_obj
                        .get("appinfo")
                        .and_then(|v| v.as_obj())
                        .and_then(|o| o.get("depots"))
                })
            };

            if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
                for (key, value) in depots.iter() {
                    if let Ok(d_id) = key.parse::<u64>() {
                        let name = value
                            .as_obj()
                            .and_then(|o| o.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or(&format!("Depot {d_id}"))
                            .to_string();

                        let size = value
                            .as_obj()
                            .and_then(|o| o.get("maxsize"))
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(0);

                        let mut config_parts = Vec::new();
                        if let Some(config) = value.as_obj().and_then(|o| o.get("config")).and_then(|v| v.as_obj()) {
                            if let Some(os) = config.get("oslist").and_then(|v| v.as_str()) {
                                config_parts.push(format!("os: {}", os));
                            }
                            if let Some(lang) = config.get("language").and_then(|v| v.as_str()) {
                                config_parts.push(format!("lang: {}", lang));
                            }
                        }

                        out.push(DepotInfo {
                            id: d_id,
                            name,
                            size,
                            file_count: 0, // Not easily available in PICS VDF without manifest
                            config: config_parts.join(", "),
                            is_owned: None,
                        });
                    }
                }
            }
        }

        out.sort_by_key(|d| d.id);
        Ok(out)
    }

    /// Estimate the download and on-disk size of installing `app_id` on `platform`,
    /// without fetching any manifests. Reads each depot's `manifests.public.size`
    /// (disk) and `manifests.public.download` (compressed) from PICS appinfo and
    /// sums the depots that match the target platform — mirroring the install
    /// pipeline's [`should_keep_depot`] selection. DLC depots (`dlcappid`) are
    /// excluded, so this estimates the base game; DLC sizing isn't covered.
    pub async fn estimate_install_size(
        &self,
        app_id: u32,
        platform: DepotPlatform,
    ) -> Result<InstallSizeEstimate> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut request = CMsgClientPICSProductInfoRequest::new();
        request
            .apps
            .push(cmsg_client_picsproduct_info_request::AppInfo {
                appid: Some(app_id),
                ..Default::default()
            });

        let response: CMsgClientPICSProductInfoResponse = connection
            .job(request)
            .await
            .context("failed requesting appinfo product info for size estimate")?;

        let app = response
            .apps
            .iter()
            .find(|entry| entry.appid() == app_id)
            .ok_or_else(|| anyhow!("missing appinfo payload for app {app_id}"))?;

        let mut est = InstallSizeEstimate::default();
        let vdf = find_vdf_in_pics(app.buffer()).context("failed to parse product info VDF")?;
        let root_obj = vdf.as_obj().context("root is not an object")?;
        let depots_val = if vdf.key() == "appinfo" || vdf.key() == app_id.to_string() {
            root_obj.get("depots")
        } else {
            root_obj.get("depots").or_else(|| {
                root_obj
                    .get("appinfo")
                    .and_then(|v| v.as_obj())
                    .and_then(|o| o.get("depots"))
            })
        };

        if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
            for (key, value) in depots.iter() {
                // Only numeric keys are depots (skip `branches`, `overflowstorage`, …).
                if key.parse::<u64>().is_err() {
                    continue;
                }
                let Some(obj) = value.as_obj() else { continue };

                // Exclude DLC content depots (estimate is for the base game).
                if obj.get("dlcappid").is_some() {
                    continue;
                }

                // Platform filter, matching the install pipeline.
                let oslist = obj
                    .get("config")
                    .and_then(|v| v.as_obj())
                    .and_then(|c| c.get("oslist"))
                    .and_then(|v| v.as_str());
                if !should_keep_depot(oslist, platform) {
                    continue;
                }

                let public = obj
                    .get("manifests")
                    .and_then(|v| v.as_obj())
                    .and_then(|m| m.get("public"))
                    .and_then(|v| v.as_obj());

                let disk = public
                    .and_then(|p| p.get("size"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
                    .or_else(|| {
                        obj.get("maxsize")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse::<u64>().ok())
                    })
                    .unwrap_or(0);
                // Steam's `download` is the compressed transfer size; fall back to the
                // uncompressed size when a depot doesn't advertise it.
                let download = public
                    .and_then(|p| p.get("download"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(disk);

                if disk > 0 || download > 0 {
                    est.disk_size += disk;
                    est.download_size += download;
                    est.depot_count += 1;
                }
            }
        }

        Ok(est)
    }

    /// List a game's launch options from its PICS `config/launch` table — the set
    /// of executables/arguments Steam can start the game with, plus their platform
    /// constraints. Read with the binary-safe VDF path (works for both binary and
    /// text PICS payloads). Entry `"0"` is sorted first (the default).
    pub async fn fetch_launch_options(&self, app_id: u32) -> Result<Vec<LaunchOptionInfo>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut request = CMsgClientPICSProductInfoRequest::new();
        request
            .apps
            .push(cmsg_client_picsproduct_info_request::AppInfo {
                appid: Some(app_id),
                ..Default::default()
            });

        let response: CMsgClientPICSProductInfoResponse = connection
            .job(request)
            .await
            .context("failed requesting appinfo product info for launch options")?;

        let app = response
            .apps
            .iter()
            .find(|entry| entry.appid() == app_id)
            .ok_or_else(|| anyhow!("missing appinfo payload for app {app_id}"))?;

        let vdf = find_vdf_in_pics(app.buffer()).context("failed to parse product info VDF")?;
        let root_obj = vdf.as_obj().context("root is not an object")?;

        // `config` sits at the root or under the numeric/"appinfo" wrapper.
        let config = root_obj.get("config").and_then(|v| v.as_obj()).or_else(|| {
            root_obj
                .get("appinfo")
                .and_then(|v| v.as_obj())
                .and_then(|o| o.get("config"))
                .and_then(|v| v.as_obj())
        });

        let mut out = Vec::new();
        if let Some(launch) = config.and_then(|c| c.get("launch")).and_then(|v| v.as_obj()) {
            for (id, entry) in launch.iter() {
                let Some(e) = entry.as_obj() else { continue };
                let field = |k: &str| e.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
                let cfg = e.get("config").and_then(|v| v.as_obj());
                let cfg_field = |k: &str| {
                    cfg.and_then(|c| c.get(k))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                };

                out.push(LaunchOptionInfo {
                    id: id.to_string(),
                    description: field("description"),
                    executable: field("executable"),
                    arguments: field("arguments"),
                    working_dir: field("workingdir"),
                    oslist: cfg_field("oslist"),
                    osarch: cfg_field("osarch"),
                    launch_type: field("type"),
                });
            }
        }

        // Default entry ("0") first, then by id.
        out.sort_by(|a, b| match (a.id.as_str(), b.id.as_str()) {
            ("0", "0") => std::cmp::Ordering::Equal,
            ("0", _) => std::cmp::Ordering::Less,
            (_, "0") => std::cmp::Ordering::Greater,
            _ => a.id.cmp(&b.id),
        });
        Ok(out)
    }

    /// Fetch the logged-in user's achievements for a game, combining the game's
    /// achievement definitions + global rarity (`Player.GetGameAchievements`) with
    /// the user's per-achievement unlock state and time (`ClientGetUserStats`,
    /// whose binary-KV schema maps each achievement to its stat/bit). Achievements
    /// the user hasn't unlocked are returned with `unlocked = false`.
    pub async fn fetch_achievements(
        &self,
        appid: u32,
        language: &str,
    ) -> Result<Vec<GameAchievement>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        let steam_id = self
            .steam_id()
            .context("not logged in — achievements need an authenticated session")?;

        // 1. Definitions + global rarity (localized).
        let mut def_req = CPlayer_GetGameAchievements_Request::new();
        def_req.set_appid(appid);
        def_req.set_language(language.to_string());
        let def_resp: CPlayer_GetGameAchievements_Response = connection
            .service_method(def_req)
            .await
            .context("Player.GetGameAchievements failed")?;

        // 2. The user's unlock state. A user who never launched the game returns no
        //    blocks (everything stays locked) — not an error.
        let mut stats_req = CMsgClientGetUserStats::new();
        stats_req.set_game_id(u64::from(appid));
        stats_req.set_steam_id_for_user(steam_id);
        let stats_resp: CMsgClientGetUserStatsResponse = connection
            .job(stats_req)
            .await
            .context("ClientGetUserStats failed")?;

        // api-name -> (stat_id, bit), parsed from the binary-KV schema.
        let bit_index = parse_achievement_schema(stats_resp.schema());
        // Case-insensitive fallback (some games' definition vs schema names differ in case).
        let bit_index_ci: HashMap<String, (u32, u32)> = bit_index
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), *v))
            .collect();
        // stat_id -> unlock_time vector (indexed by bit).
        let mut unlock_by_stat: HashMap<u32, &Vec<u32>> = HashMap::new();
        for block in &stats_resp.achievement_blocks {
            unlock_by_stat.insert(block.achievement_id(), &block.unlock_time);
        }
        tracing::debug!(
            appid,
            eresult = stats_resp.eresult(),
            schema_bytes = stats_resp.schema().len(),
            schema_achievements = bit_index.len(),
            unlock_blocks = unlock_by_stat.len(),
            "achievements: parsed user-stats schema"
        );
        if !stats_resp.schema().is_empty() && bit_index.is_empty() {
            tracing::warn!(
                appid,
                schema_bytes = stats_resp.schema().len(),
                "achievement schema present but parsed 0 entries; unlock state unavailable"
            );
        }

        let mut out = Vec::new();
        for ach in &def_resp.achievements {
            let api = ach.internal_name().to_string();
            let mapped = bit_index
                .get(&api)
                .or_else(|| bit_index_ci.get(&api.to_ascii_lowercase()))
                .copied();
            let (unlocked, unlock_time) = match mapped {
                Some((stat_id, bit)) => {
                    let t = unlock_by_stat
                        .get(&stat_id)
                        .and_then(|times| times.get(bit as usize))
                        .copied()
                        .unwrap_or(0);
                    if t > 0 { (true, Some(t)) } else { (false, None) }
                }
                None => (false, None),
            };

            out.push(GameAchievement {
                name: ach.localized_name().to_string(),
                description: ach.localized_desc().to_string(),
                hidden: ach.hidden(),
                icon_unlocked: achievement_icon_url(appid, ach.icon()),
                icon_locked: achievement_icon_url(appid, ach.icon_gray()),
                global_percent: ach.player_percent_unlocked().parse::<f32>().unwrap_or(0.0),
                unlocked,
                unlock_time,
                api_name: api,
            });
        }
        Ok(out)
    }

    pub async fn get_depot_key(&self, app_id: u32, depot_id: u32) -> Result<Vec<u8>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        let mut request = CMsgClientGetDepotDecryptionKey::new();
        request.set_depot_id(depot_id);
        request.set_app_id(app_id);

        let response: CMsgClientGetDepotDecryptionKeyResponse = connection.job(request).await?;
        if response.eresult() != 1 {
            bail!(
                "failed to get depot key for depot {depot_id}: eresult {}",
                response.eresult()
            );
        }

        Ok(response.depot_encryption_key().to_vec())
    }

    pub async fn verify_depot_ownership(&self, app_id: u32, depot_ids: Vec<u64>) -> HashMap<u64, bool> {
        tracing::info!("Verifying ownership for {} depots...", depot_ids.len());
        let mut results = HashMap::new();

        let connection = match self.connection.as_ref() {
            Some(c) => c,
            None => {
                for id in depot_ids { results.insert(id, false); }
                return results;
            }
        };

        // 1. Ensure we have an App Ticket (Warm up session)
        let _ = self.get_app_ticket(app_id).await;

        for depot_id in depot_ids {
            let mut request = CMsgClientGetDepotDecryptionKey::new();
            request.set_depot_id(depot_id as u32);
            request.set_app_id(app_id);

            match connection.job(request).await {
                Ok(response) => {
                    let response: CMsgClientGetDepotDecryptionKeyResponse = response;
                    if response.eresult() == 1 { // EResult::OK
                        results.insert(depot_id, true);
                    } else {
                        results.insert(depot_id, false);
                    }
                }
                Err(_) => {
                    results.insert(depot_id, false);
                }
            }
        }
        results
    }

    pub async fn fetch_depots(&self, appid: u32) -> Result<Vec<BrowserDepotInfo>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        depot_browser::fetch_depots(connection, appid).await
    }

    pub async fn fetch_manifest_files(
        &self,
        appid: u32,
        depot_id: u32,
        manifest_ref: &str,
    ) -> Result<Vec<ManifestFileEntry>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        depot_browser::fetch_manifest_files(connection, appid, depot_id, manifest_ref).await
    }

    pub fn download_single_file(
        &self,
        appid: u32,
        depot_id: u32,
        manifest_ref: &str,
        file_path: &str,
        output_dir: &Path,
    ) -> Result<()> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        depot_browser::download_single_file(
            connection,
            appid,
            depot_id,
            manifest_ref,
            file_path,
            output_dir,
        )
    }

    pub async fn fetch_owned_games(&mut self) -> Result<Vec<OwnedGame>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let request = CPlayer_GetOwnedGames_Request {
            steamid: Some(u64::from(connection.steam_id())),
            include_appinfo: Some(true),
            include_played_free_games: Some(true),
            ..Default::default()
        };

        tracing::debug!("Calling Player.GetOwnedGames ...");
        let response: CPlayer_GetOwnedGames_Response = connection
            .service_method(request)
            .await
            .context("failed calling Player.GetOwnedGames")?;
        tracing::debug!("Player.GetOwnedGames returned {} games", response.games.len());

        let mut owned = Vec::new();
        for game in response.games {
            owned.push(OwnedGame {
                app_id: game.appid() as u32,
                name: if game.name().is_empty() {
                    format!("App {}", game.appid())
                } else {
                    game.name().to_string()
                },
                playtime_forever_minutes: game.playtime_forever() as u32,
                local_manifest_ids: HashMap::new(),
                update_available: false,
            });
        }

        save_library_cache(&owned).await.ok();
        Ok(owned)
    }

    /// Fetch games available to this account through Steam Family Sharing that the
    /// account does **not** itself own. Returns an empty list if the account is not
    /// part of a family group. These may or may not be installed locally.
    pub async fn fetch_family_shared_apps(&self) -> Result<Vec<SharedApp>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        let my_steamid = u64::from(connection.steam_id());

        // 1. Resolve the family group this account belongs to.
        let mut group_req = CFamilyGroups_GetFamilyGroupForUser_Request::new();
        group_req.set_steamid(my_steamid);
        group_req.set_include_family_group_response(true);

        tracing::debug!("Calling FamilyGroups.GetFamilyGroupForUser ...");
        let group_resp: CFamilyGroups_GetFamilyGroupForUser_Response = connection
            .service_method(group_req)
            .await
            .context("failed calling FamilyGroups.GetFamilyGroupForUser")?;

        let family_groupid = group_resp.family_groupid();
        if family_groupid == 0 {
            // Account is not in a family group; nothing is shared with it.
            return Ok(Vec::new());
        }

        // 2. List apps shared with us by other family members (exclude our own).
        let mut apps_req = CFamilyGroups_GetSharedLibraryApps_Request::new();
        apps_req.set_family_groupid(family_groupid);
        apps_req.set_steamid(my_steamid);
        apps_req.set_include_own(false);
        apps_req.set_include_excluded(false);
        apps_req.set_include_non_games(false);
        apps_req.set_max_apps(10_000);
        apps_req.set_language("english".to_string());

        let apps_resp: CFamilyGroups_GetSharedLibraryApps_Response = connection
            .service_method(apps_req)
            .await
            .context("failed calling FamilyGroups.GetSharedLibraryApps")?;

        let mut shared = Vec::new();
        for app in apps_resp.apps {
            let app_id = app.appid();
            shared.push(SharedApp {
                app_id,
                name: if app.name().is_empty() {
                    format!("App {app_id}")
                } else {
                    app.name().to_string()
                },
                owner_steamid: app.owner_steamids.first().copied(),
            });
        }
        Ok(shared)
    }

    pub async fn refresh_owned_games(&mut self, _session: &SessionState) -> Result<Vec<OwnedGame>> {
        self.fetch_owned_games().await
    }

    pub async fn load_cached_owned_games(&self) -> Result<Vec<OwnedGame>> {
        load_library_cache().await
    }

    pub async fn check_for_updates(&self, games: &mut [LibraryGame]) -> Result<()> {
        for game in games.iter_mut() {
            game.update_available = false;
            game.local_manifest_ids.clear();

            if !game.is_installed {
                continue;
            }

            let (local, branch) = self.local_manifest_info(game)?;
            game.local_manifest_ids = local.clone();
            game.active_branch = branch;

            if self.is_offline() || self.connection.is_none() {
                continue;
            }

            let remote = self
                .remote_manifest_ids(game.app_id, &game.active_branch)
                .await
                .unwrap_or_default();
            if remote.is_empty() {
                continue;
            }

            game.update_available = remote.iter().any(|(depot, remote_manifest)| {
                local.get(depot).copied().unwrap_or_default() != *remote_manifest
            });
        }

        Ok(())
    }

    fn local_manifest_info(&self, game: &LibraryGame) -> Result<(HashMap<u64, u64>, String)> {
        let install_path = match &game.install_path {
            Some(path) => PathBuf::from(path),
            None => return Ok((HashMap::new(), "public".to_string())),
        };

        let steamapps = match install_path.parent().and_then(|p| p.parent()) {
            Some(path) => path.to_path_buf(),
            None => return Ok((HashMap::new(), "public".to_string())),
        };

        let manifest_path = steamapps.join(format!("appmanifest_{}.acf", game.app_id));
        if !manifest_path.exists() {
            return Ok((HashMap::new(), "public".to_string()));
        }

        let raw = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed reading {}", manifest_path.display()))?;
        let manifests = parse_installed_depots_from_acf(&raw);
        let branch = parse_active_branch_from_acf(&raw);
        Ok((manifests, branch))
    }

    async fn remote_manifest_ids(&self, appid: u32, branch: &str) -> Result<HashMap<u64, u64>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        SteamClient::remote_manifest_ids_static(connection, appid, branch).await
    }

    pub async fn get_user_profile(&self, current_library_len: usize) -> Result<UserProfile> {
        let persisted = load_session().await.unwrap_or_default();
        let account_name = persisted
            .account_name
            .unwrap_or_else(|| "Unknown User".to_string());

        if self.is_offline() {
            let cached_games = load_library_cache().await.unwrap_or_default();
            return Ok(UserProfile {
                steam_id: persisted.steam_id.unwrap_or_default(),
                account_name,
                game_count: cached_games.len(),
                is_online: false,
            });
        }

        let steam_id = self
            .connection
            .as_ref()
            .map(|connection| u64::from(connection.steam_id()))
            .or(persisted.steam_id)
            .unwrap_or_default();

        Ok(UserProfile {
            steam_id,
            account_name,
            game_count: current_library_len,
            is_online: true,
        })
    }

    pub async fn get_extended_app_info(&self, appid: u32) -> Result<ExtendedAppInfo> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut request = CMsgClientPICSProductInfoRequest::new();
        request
            .apps
            .push(cmsg_client_picsproduct_info_request::AppInfo {
                appid: Some(appid),
                ..Default::default()
            });

        let response: CMsgClientPICSProductInfoResponse = connection
            .job(request)
            .await
            .context("failed requesting appinfo product info for extended metadata")?;

        let app = response
            .apps
            .iter()
            .find(|entry| entry.appid() == appid)
            .ok_or_else(|| anyhow!("missing appinfo payload for app {appid}"))?;

        let raw_vdf = String::from_utf8_lossy(app.buffer()).to_string();
        let parsed: AppInfoRoot =
            parse_appinfo(&raw_vdf).context("failed to parse product info VDF")?;

        let common = parsed
            .appinfo
            .as_ref()
            .and_then(|a| a.common.as_ref())
            .or(parsed.common.as_ref());

        let name = common.and_then(|c| c.name.clone());

        let dlcs: Vec<u32> = common
            .map(|c| {
                c.dlc
                    .keys()
                    .filter_map(|k| k.parse::<u32>().ok())
                    .collect()
            })
            .unwrap_or_default();

        let depots_map = parsed
            .appinfo
            .as_ref()
            .map(|a| &a.depots)
            .unwrap_or(&parsed.depots);
        let mut depots = Vec::new();
        for (id_str, node) in depots_map {
            let is_digit = id_str.chars().all(|c| c.is_ascii_digit());
            if is_digit {
                let id = id_str.parse::<u32>().unwrap_or(0);
                let name = node
                    ._other
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown Depot")
                    .to_string();
                depots.push((id, name));
            }
        }

        let config = parsed
            .appinfo
            .as_ref()
            .and_then(|a| a.config.as_ref())
            .or(parsed.config.as_ref());

        let mut launch_options = Vec::new();
        if let Some(config) = config {
            for entry in config.launch.values() {
                launch_options.push(RawLaunchOption {
                    executable: entry.executable.clone().unwrap_or_default(),
                    arguments: entry.arguments.clone().unwrap_or_default(),
                });
            }
        }

        let manifest_path = self.appmanifest_path(appid).await?;
        let active_branch = if manifest_path.exists() {
            let raw = std::fs::read_to_string(&manifest_path).unwrap_or_default();
            parse_active_branch_from_acf(&raw)
        } else {
            "public".to_string()
        };

        Ok(ExtendedAppInfo {
            name,
            dlcs,
            depots,
            launch_options,
            active_branch,
        })
    }

    /// Fetch a single app's PICS appinfo and infer whether it appears to require
    /// an online connection to play. Steam exposes no explicit flag for this, so
    /// the answer is derived from the app's store categories — see
    /// [`category_online_required`]. Requires an active Steam connection.
    pub async fn fetch_online_required(&self, appid: u32) -> Result<bool> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut request = CMsgClientPICSProductInfoRequest::new();
        request
            .apps
            .push(cmsg_client_picsproduct_info_request::AppInfo {
                appid: Some(appid),
                ..Default::default()
            });

        let response: CMsgClientPICSProductInfoResponse = connection
            .job(request)
            .await
            .context("failed requesting appinfo product info for online-required check")?;

        let app = response
            .apps
            .iter()
            .find(|entry| entry.appid() == appid)
            .ok_or_else(|| anyhow!("missing appinfo payload for app {appid}"))?;

        // PICS product-info buffers are usually *binary* VDF (text only for some
        // apps), so go through `find_vdf_in_pics` rather than the text-only
        // `parse_appinfo`. The category map lives at `<root>/common/category`,
        // where the root key is either "appinfo" or the numeric appid.
        let vdf = find_vdf_in_pics(app.buffer()).context("failed to parse product info VDF")?;
        let root_obj = vdf.as_obj().context("PICS root is not an object")?;
        let common = if vdf.key() == "appinfo" || vdf.key() == appid.to_string() {
            root_obj.get("common")
        } else {
            root_obj.get("common").or_else(|| {
                root_obj
                    .values()
                    .next()
                    .and_then(|v| v.as_obj())
                    .and_then(|o| o.get("common"))
            })
        };

        let mut categories = HashMap::new();
        if let Some(cat_obj) = common
            .and_then(|c| c.as_obj())
            .and_then(|o| o.get("category"))
            .and_then(|v| v.as_obj())
        {
            for (key, value) in cat_obj.iter() {
                if let Some(v) = value.as_str() {
                    categories.insert(key.to_string(), v.to_string());
                }
            }
        }

        Ok(category_online_required(&categories))
    }

    /// Fetch human-facing store metadata for one or more apps via the
    /// `StoreBrowse.GetItems` service method (over the CM connection — no HTTPS
    /// storefront API). Returns one [`StoreAppInfo`] per app the store knows
    /// about; unknown/region-locked ids are simply omitted. Requires a connection.
    pub async fn fetch_store_apps(&self, app_ids: &[u32]) -> Result<Vec<StoreAppInfo>> {
        if app_ids.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut data = StoreBrowseItemDataRequest::new();
        data.set_include_basic_info(true);
        data.set_include_release(true);
        data.set_include_platforms(true);
        data.set_include_reviews(true);
        data.set_include_all_purchase_options(true);
        data.set_include_full_description(true);
        data.set_include_assets(true);

        let mut context = StoreBrowseContext::new();
        context.set_language("english".to_string());
        context.set_country_code("US".to_string());

        let mut request = CStoreBrowse_GetItems_Request::new();
        request.context = MessageField::some(context);
        request.data_request = MessageField::some(data);
        for &id in app_ids {
            let mut item_id = StoreItemID::new();
            item_id.set_appid(id);
            request.ids.push(item_id);
        }

        let response: CStoreBrowse_GetItems_Response = connection
            .service_method(request)
            .await
            .context("failed calling StoreBrowse.GetItems")?;

        Ok(response
            .store_items
            .iter()
            .filter(|item| item.appid() != 0)
            .map(store_item_to_app_info)
            .collect())
    }

    pub async fn get_product_info(&mut self, appid: u32, prefer_proton: bool) -> Result<Vec<LaunchInfo>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut request = CMsgClientPICSProductInfoRequest::new();
        request
            .apps
            .push(cmsg_client_picsproduct_info_request::AppInfo {
                appid: Some(appid),
                ..Default::default()
            });

        let response: CMsgClientPICSProductInfoResponse = connection
            .job(request)
            .await
            .context("failed requesting appinfo product info for launch metadata")?;

        let app = response
            .apps
            .iter()
            .find(|entry| entry.appid() == appid)
            .ok_or_else(|| anyhow!("missing appinfo payload for app {appid}"))?;

        let raw_vdf = String::from_utf8_lossy(app.buffer()).to_string();
        if raw_vdf.trim().is_empty() {
            bail!("empty appinfo payload returned for app {appid}")
        }

        let launch_infos = parse_launch_info_from_vdf(appid, &raw_vdf, prefer_proton)
            .context("failed to parse launch metadata from PICS appinfo")?;

        Ok(launch_infos)
    }

    pub async fn play_game(
        &mut self,
        app: &LibraryGame,
        proton_path: Option<&str>,
        user_config: Option<&crate::models::UserAppConfig>,
        force_windows: bool,
    ) -> Result<LaunchInfo> {
        let prefer_proton = force_windows || proton_path.is_some();
        let launch_options = self.get_product_info(app.app_id, prefer_proton).await?;
        // When forcing a Windows launch, prefer a Windows executable entry.
        let launch_info = if force_windows {
            launch_options
                .iter()
                .find(|o| o.target == LaunchTarget::WindowsProton)
                .or_else(|| launch_options.first())
                .cloned()
        } else {
            launch_options.first().cloned()
        }
        .ok_or_else(|| anyhow!("no launch options"))?;

        let launcher_config = load_launcher_config().await.unwrap_or_default();

        // Proton/Wine only exists on Linux. On Windows, a Windows game runs natively, so
        // run its executable directly instead of routing through the Proton pipeline.
        let native_windows = force_windows
            || (cfg!(target_os = "windows") && launch_info.target == LaunchTarget::WindowsProton);

        let chosen_proton_path = if native_windows {
            None
        } else {
            match launch_info.target {
                LaunchTarget::NativeLinux => None,
                LaunchTarget::WindowsProton => {
                    proton_path.or(Some(launcher_config.proton_version.as_str()))
                }
            }
        };

        let cloud_enabled = launcher_config.enable_cloud_sync && !self.is_offline();
        let mut cloud_client = None;
        let mut local_root = None;

        if cloud_enabled {
            let client = CloudClient::new(
                self.connection
                    .as_ref()
                    .cloned()
                    .context("steam connection not initialized")?,
            );
            let root = default_cloud_root(client.steam_id(), app.app_id)?;
            tracing::info!(appid = app.app_id, path = %root.display(), "Syncing Cloud...");
            let _ = client.sync_down(app.app_id, &root).await;
            cloud_client = Some(client);
            local_root = Some(root);
        }

        let mut child = if native_windows {
            self.spawn_windows_native(app, &launch_info, user_config).await?
        } else {
            self.spawn_game_process(app, &launch_info, chosen_proton_path, &launcher_config, user_config).await?
        };

        // Record the launch so a separate `aurelia stop <app_id>` invocation can
        // find and terminate the process while we block on `wait()` below.
        let wineprefix = if native_windows {
            None
        } else {
            let user_configs = crate::config::load_user_configs().await.unwrap_or_default();
            let pfx = crate::utils::steam_wineprefix_for_game(&launcher_config, app.app_id, &user_configs);
            // Only record a per-game (compatdata) prefix — sweeping the shared
            // master prefix on stop would also kill the Steam client inside it.
            pfx.to_string_lossy().contains("compatdata").then_some(pfx)
        };
        let record = crate::running::RunningGame {
            app_id: app.app_id,
            name: app.name.clone(),
            pid: child.id(),
            wineprefix,
        };
        if let Err(e) = crate::running::record_launch(&record) {
            tracing::warn!(appid = app.app_id, "could not record running game: {e:#}");
        }

        let wait_result = child.wait().context("failed waiting for game process exit");
        crate::running::clear(app.app_id);
        wait_result?;

        if cloud_enabled {
            if let (Some(client), Some(root)) = (cloud_client.as_ref(), local_root.as_ref()) {
                // The game has already run and exited, so a cloud-upload failure must not
                // be surfaced as a launch failure. Log it and continue (this mirrors the
                // best-effort sync_down before launch).
                match client.sync_up(app.app_id, root).await {
                    Ok(()) => tracing::info!(appid = app.app_id, "Upload Complete"),
                    Err(e) => {
                        tracing::warn!(appid = app.app_id, "Cloud upload failed (continuing): {e:#}")
                    }
                }
            }
        }

        Ok(launch_info)
    }

    pub async fn launch_game(
        &self,
        app: &LibraryGame,
        launch_info: &LaunchInfo,
        proton_path: Option<&str>,
        user_config: Option<&crate::models::UserAppConfig>,
    ) -> Result<()> {
        let launcher_config = load_launcher_config().await.unwrap_or_default();
        self.spawn_game_process(app, launch_info, proton_path, &launcher_config, user_config).await?;
        Ok(())
    }

    pub async fn update_game(
        &self,
        appid: u32,
        shared_state: Arc<std::sync::RwLock<crate::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        self.start_manifest_download(appid, false, shared_state)
            .await
    }

    pub async fn verify_game(
        &self,
        appid: u32,
        shared_state: Arc<std::sync::RwLock<crate::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        self.start_manifest_download(appid, true, shared_state)
            .await
    }

    async fn start_manifest_download(
        &self,
        appid: u32,
        verify_mode: bool,
        shared_state: Arc<std::sync::RwLock<crate::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        let connection = self
            .connection
            .as_ref()
            .cloned()
            .context("steam connection not initialized")?;

        let install_root = self.install_root_for_app(appid).await?;
        let manifest_path = self.appmanifest_path(appid).await?;
        let (tx, rx) = tokio::sync::mpsc::channel(128);

        let (local_manifests, active_branch) = self
            .local_manifest_info_for_appid(appid)
            .await
            .unwrap_or_else(|_| (HashMap::new(), "public".to_string()));

        let client_clone = self.clone();
        let shared_state_clone = shared_state.clone();
        let game_name = self.resolve_install_game_name(appid).await;
        tokio::task::spawn(async move {
            if let Ok(mut state) = shared_state_clone.write() {
                state.is_downloading = true;
                state.is_paused = false;
                state.app_id = appid;
                state.app_name = game_name.clone();
                state.downloaded_bytes = 0;
                state.status_text = format!("Preparing operation for {}...", game_name);
            }

            let _ = tx
                .send(DownloadProgress {
                    state: DownloadProgressState::Queued,
                    current_file: if verify_mode {
                        "verifying installed chunks".to_string()
                    } else {
                        "resolving latest manifest".to_string()
                    },
                    ..Default::default()
                })
                .await;

            let remote_manifests = if verify_mode {
                local_manifests.clone()
            } else {
                SteamClient::remote_manifest_ids_static(&connection, appid, &active_branch)
                    .await
                    .unwrap_or_default()
            };

            let mut selections = Vec::new();
            for (depot_id, manifest_id) in &remote_manifests {
                selections.push(ManifestSelection {
                    app_id: appid,
                    depot_id: *depot_id as u32,
                    manifest_id: *manifest_id,
                    appinfo_vdf: String::new(),
                });
            }

            if selections.is_empty() {
                let _ = tx
                    .send(DownloadProgress {
                        state: DownloadProgressState::Failed,
                        current_file: "no manifest/depot available for download".to_string(),
                        ..Default::default()
                    })
                    .await;
                return;
            };

            let hosts = match client_clone.get_content_servers(connection.cell_id()).await {
                Ok(h) => h,
                Err(e) => {
                    let _ = tx
                        .send(DownloadProgress {
                            state: DownloadProgressState::Failed,
                            current_file: format!("Failed to fetch content servers: {}", e),
                            ..Default::default()
                        })
                        .await;
                    return;
                }
            };

            let mut success = true;
            let mut successful_depots = Vec::new();

            for selection in selections {
                let key: Vec<u8> = match client_clone.get_depot_key(appid, selection.depot_id).await {
                    Ok(k) => k,
                    Err(e) => {
                        tracing::warn!(
                            "Skipping Depot {} (No Key/Not Owned): {}",
                            selection.depot_id,
                            e
                        );
                        continue;
                    }
                };

                let manifest_code: Option<u64> = client_clone
                    .get_manifest_request_code(appid, selection.depot_id, selection.manifest_id)
                    .await
                    .ok();

                let mut depot_success = false;
                for host in &hosts {
                    let token: Option<String> = client_clone
                        .get_cdn_auth_token(appid, selection.depot_id, host)
                        .await
                        .ok();

                    let (host_name, port) = if let Some(pos) = host.find(':') {
                        (
                            &host[..pos],
                            host[pos + 1..].parse::<u16>().unwrap_or(80),
                        )
                    } else {
                        (host.as_str(), 80)
                    };

                    let cdn_server = steam_cdn::web_api::content_service::CDNServer {
                        r#type: "CDN".to_string(),
                        https: port == 443,
                        host: host_name.to_string(),
                        vhost: host_name.to_string(),
                        port,
                        cell_id: connection.cell_id(),
                        load: 0,
                        weighted_load: 0,
                        auth_token: token,
                    };

                    let cdn_client = steam_cdn::CDNClient::with_server(
                        Arc::new(connection.clone()),
                        cdn_server,
                    );

                    let tx_clone = tx.clone();
                    let selection_depot_id = selection.depot_id;
                    let on_progress = Arc::new(move |bytes: u64| {
                        let _ = tx_clone.try_send(DownloadProgress {
                            state: if verify_mode {
                                DownloadProgressState::Verifying
                            } else {
                                DownloadProgressState::Downloading
                            },
                            bytes_downloaded: bytes,
                            depot_id: selection_depot_id,
                            depot_bytes_downloaded: bytes,
                            current_file: format!("Depot {}", selection_depot_id),
                            ..Default::default()
                        });
                    });

                    let depot_size = Arc::new(std::sync::atomic::AtomicU64::new(0));
                    let size_clone = depot_size.clone();
                    let on_manifest = Arc::new(move |total_bytes: u64| {
                        size_clone.store(total_bytes, std::sync::atomic::Ordering::SeqCst);
                    });

                    let abort_signal = shared_state_clone
                        .read()
                        .ok()
                        .map(|s| s.abort_signal.clone());

                    match cdn_client
                        .download_depot(
                            appid,
                            selection.depot_id,
                            selection.manifest_id,
                            &key,
                            &install_root,
                            manifest_code,
                            verify_mode,
                            abort_signal,
                            Some(on_progress),
                            Some(on_manifest),
                        )
                        .await
                    {
                        Ok(_) => {
                            let aborted = shared_state_clone.read()
                                .map(|s| s.abort_signal.load(std::sync::atomic::Ordering::Relaxed))
                                .unwrap_or(false);
                            if aborted {
                                break;
                            }

                            depot_success = true;
                            successful_depots.push((
                                selection.depot_id,
                                selection.manifest_id,
                                depot_size.load(std::sync::atomic::Ordering::SeqCst),
                            ));
                            break;
                        }
                        Err(e) => {
                            tracing::error!("CDN Error from {}: {}", host, e);
                        }
                    }
                }

                if !depot_success {
                    let aborted = shared_state_clone.read()
                        .map(|s| s.abort_signal.load(std::sync::atomic::Ordering::Relaxed))
                        .unwrap_or(false);

                    if aborted {
                        success = false;
                        break;
                    }

                    let _ = tx
                        .send(DownloadProgress {
                            state: DownloadProgressState::Failed,
                            current_file: format!(
                                "Failed to download/verify depot {} from all servers",
                                selection.depot_id
                            ),
                            ..Default::default()
                        })
                        .await;
                    success = false;
                    break;
                }
            }

            if success {
                if let Ok(mut state) = shared_state_clone.write() {
                    state.is_downloading = false;
                    state.status_text = "Operation complete".to_string();
                }

                let (game_name, pics_installdir) = client_clone.resolve_install_game_info(appid).await;
                let installdir = pics_installdir.unwrap_or_else(|| sanitize_install_dir(&game_name));
                // Record the current build so Steam sees the install as up to date.
                let build_id =
                    SteamClient::remote_buildid_static(&connection, appid, &active_branch).await;

                if let Err(err) = SteamClient::write_appmanifest(
                    &manifest_path,
                    appid,
                    &game_name,
                    &installdir,
                    successful_depots,
                    build_id.as_deref(),
                    true,
                ) {
                    tracing::warn!("failed writing appmanifest for {}: {}", appid, err);
                }
                let _ = tx
                    .send(DownloadProgress {
                        state: DownloadProgressState::Completed,
                        bytes_downloaded: 1,
                        total_bytes: 1,
                        current_file: if verify_mode {
                            "verify completed".to_string()
                        } else {
                            "update completed".to_string()
                        },
                        ..Default::default()
                    })
                    .await;
            } else {
                if let Ok(mut state) = shared_state_clone.write() {
                    state.is_downloading = false;
                    state.status_text = "Operation failed or paused".to_string();
                }
            }
        });

        Ok(rx)
    }

    async fn appmanifest_path(&self, appid: u32) -> Result<PathBuf> {
        let file = format!("appmanifest_{appid}.acf");

        // Search every known Steam library (incl. other drives) for the manifest.
        for root in crate::library::all_library_roots().await {
            let candidate = root.join("steamapps").join(&file);
            if candidate.exists() {
                return Ok(candidate);
            }
        }

        // Fall back to the configured library root even if the manifest is absent,
        // preserving the previous behaviour for callers that tolerate a missing file.
        let cfg = load_launcher_config().await?;
        Ok(PathBuf::from(cfg.steam_library_path)
            .join("steamapps")
            .join(file))
    }

    async fn local_manifest_info_for_appid(&self, appid: u32) -> Result<(HashMap<u64, u64>, String)> {
        let manifest_path = self.appmanifest_path(appid).await?;
        if !manifest_path.exists() {
            return Ok((HashMap::new(), "public".to_string()));
        }
        let raw = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed reading {}", manifest_path.display()))?;
        let manifests = parse_installed_depots_from_acf(&raw);
        let branch = parse_active_branch_from_acf(&raw);
        Ok((manifests, branch))
    }

    async fn install_root_for_app(&self, appid: u32) -> Result<PathBuf> {
        let manifest_path = self.appmanifest_path(appid).await?;
        let steamapps = manifest_path
            .parent()
            .ok_or_else(|| anyhow!("invalid steamapps path for app {appid}"))?
            .to_path_buf();

        if manifest_path.exists() {
            let raw = std::fs::read_to_string(&manifest_path)
                .with_context(|| format!("failed reading {}", manifest_path.display()))?;
            if let Some(installdir) = parse_installdir_from_acf(&raw) {
                let p = steamapps.join("common").join(&installdir);
                if p.exists() {
                    return Ok(p);
                }

                // Fallback: search for app id markers if the specified installdir doesn't exist
                if let Some(fallback) = self.probe_install_dir_by_appid(&steamapps, appid) {
                    tracing::info!("Found fallback install dir for app {appid}: {:?}", fallback);
                    return Ok(fallback);
                }

                // Even if it doesn't exist, we return the path it *should* be at
                return Ok(p);
            }
        }

        // Final fallback if no manifest or installdir
        Ok(PathBuf::from(load_launcher_config().await?.steam_library_path)
            .join("steamapps")
            .join("common")
            .join(appid.to_string()))
    }

    fn probe_install_dir_by_appid(&self, steamapps: &Path, appid: u32) -> Option<PathBuf> {
        let common = steamapps.join("common");
        if !common.exists() {
            return None;
        }

        let appid_str = appid.to_string();

        if let Ok(entries) = std::fs::read_dir(common) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Check for steam_appid.txt
                    let appid_txt = path.join("steam_appid.txt");
                    if appid_txt.exists() {
                        if let Ok(content) = std::fs::read_to_string(appid_txt) {
                            if content.trim() == appid_str {
                                return Some(path);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    async fn remote_manifest_ids_static(
        connection: &Connection,
        appid: u32,
        branch: &str,
    ) -> Result<HashMap<u64, u64>> {
        let mut request = CMsgClientPICSProductInfoRequest::new();
        request
            .apps
            .push(cmsg_client_picsproduct_info_request::AppInfo {
                appid: Some(appid),
                ..Default::default()
            });

        let response: CMsgClientPICSProductInfoResponse = connection
            .job(request)
            .await
            .context("failed requesting appinfo product info for update metadata")?;

        let app = response
            .apps
            .iter()
            .find(|entry| entry.appid() == appid)
            .ok_or_else(|| anyhow!("missing appinfo payload for app {appid}"))?;

        let mut manifests = HashMap::new();
        if let Ok(vdf) = find_vdf_in_pics(app.buffer()) {
            let root_obj = vdf.as_obj().unwrap();
            let depots_val = if vdf.key() == "appinfo" || vdf.key() == appid.to_string() {
                root_obj.get("depots")
            } else {
                root_obj.get("depots").or_else(|| {
                    root_obj
                        .get("appinfo")
                        .and_then(|v| v.as_obj())
                        .and_then(|o| o.get("depots"))
                })
            };

            if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
                for (key, value) in depots.iter() {
                    if let Ok(d_id) = key.parse::<u64>() {
                        if let Some(m_id) = extract_manifest_id_robust(value, branch) {
                            manifests.insert(d_id, m_id);
                        } else if branch != "public" {
                            if let Some(m_id) = extract_manifest_id_robust(value, "public") {
                                manifests.insert(d_id, m_id);
                            }
                        }
                    }
                }
            }
        }
        Ok(manifests)
    }

    /// Fetch the current build id for a branch from PICS, for recording in the
    /// appmanifest so Steam treats the install as up to date. Falls back to `public`.
    async fn remote_buildid_static(
        connection: &Connection,
        appid: u32,
        branch: &str,
    ) -> Option<String> {
        let mut request = CMsgClientPICSProductInfoRequest::new();
        request
            .apps
            .push(cmsg_client_picsproduct_info_request::AppInfo {
                appid: Some(appid),
                ..Default::default()
            });

        let response: CMsgClientPICSProductInfoResponse = connection.job(request).await.ok()?;
        let app = response.apps.iter().find(|entry| entry.appid() == appid)?;
        let vdf = find_vdf_in_pics(app.buffer()).ok()?;
        let root_obj = vdf.as_obj()?;
        let depots_val = if vdf.key() == "appinfo" || vdf.key() == appid.to_string() {
            root_obj.get("depots")
        } else {
            root_obj.get("depots").or_else(|| {
                root_obj
                    .get("appinfo")
                    .and_then(|v| v.as_obj())
                    .and_then(|o| o.get("depots"))
            })
        };

        let buildid = |b: &str| {
            depots_val
                .and_then(|d| d.get_obj(&["branches", b]))
                .and_then(|node| node.get("buildid"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };
        buildid(branch).or_else(|| buildid("public"))
    }

    pub async fn fetch_app_metadata(&self, appid: u32) -> Option<AppMetadata> {
        let url = format!("https://store.steampowered.com/api/appdetails?appids={appid}&filters=basic");
        let resp = reqwest::get(url).await.ok()?;
        let json: serde_json::Value = resp.json().await.ok()?;
        let data = json.get(appid.to_string())?.get("data")?;

        let name = data.get("name")?.as_str()?.to_string();
        let header_image = data
            .get("header_image")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Some(AppMetadata { name, header_image })
    }

    /// If `appid` is a DLC, return the base game's appid it depends on.
    /// Returns `None` for base games (or if the relationship can't be determined).
    /// Tries the authoritative PICS appinfo first, then the StoreBrowse service as
    /// a fallback (some DLC don't carry the parent reference in their PICS appinfo).
    /// Both sources go over the Steam CM connection — no storefront API.
    pub async fn resolve_dlc_parent(&self, appid: u32) -> Option<u32> {
        if let Some(base) = self.dlc_parent_from_pics(appid).await {
            return Some(base);
        }
        self.dlc_parent_from_store(appid).await
    }

    /// DLC → base-game lookup via `StoreBrowse.GetItems` (`related_items.parent_appid`).
    async fn dlc_parent_from_store(&self, appid: u32) -> Option<u32> {
        let connection = self.connection.as_ref()?;

        let mut context = StoreBrowseContext::new();
        context.set_language("english".to_string());
        context.set_country_code("US".to_string());

        let mut request = CStoreBrowse_GetItems_Request::new();
        request.context = MessageField::some(context);
        let mut item_id = StoreItemID::new();
        item_id.set_appid(appid);
        request.ids.push(item_id);

        let response: CStoreBrowse_GetItems_Response = connection.service_method(request).await.ok()?;
        let item = response.store_items.iter().find(|i| i.appid() == appid)?;
        item.related_items
            .as_ref()
            .map(|r| r.parent_appid())
            .filter(|&base| base != 0 && base != appid)
    }

    async fn dlc_parent_from_pics(&self, appid: u32) -> Option<u32> {
        let connection = self.connection.as_ref()?;

        let mut request = CMsgClientPICSProductInfoRequest::new();
        request
            .apps
            .push(cmsg_client_picsproduct_info_request::AppInfo {
                appid: Some(appid),
                ..Default::default()
            });

        let response: CMsgClientPICSProductInfoResponse = connection.job(request).await.ok()?;
        let app = response.apps.iter().find(|e| e.appid() == appid)?;
        let raw_vdf = String::from_utf8(app.buffer().to_vec()).ok()?;
        let parsed = parse_appinfo(&raw_vdf).ok()?;

        let common = parsed
            .appinfo
            .as_ref()
            .and_then(|a| a.common.as_ref())
            .or(parsed.common.as_ref());

        let is_dlc = common
            .and_then(|c| c.app_type.as_deref())
            .map(|t| t.eq_ignore_ascii_case("dlc"))
            .unwrap_or(false);
        if !is_dlc {
            return None;
        }

        let extended = parsed
            .appinfo
            .as_ref()
            .and_then(|a| a.extended.as_ref())
            .or(parsed.extended.as_ref());

        extended
            .and_then(|e| e.dependantonapp.as_deref())
            .and_then(|s| s.trim().parse::<u32>().ok())
            .or_else(|| {
                common
                    .and_then(|c| c.parent.as_deref())
                    .and_then(|s| s.trim().parse::<u32>().ok())
            })
            .filter(|&base| base != 0 && base != appid)
    }

    pub async fn resolve_install_game_info(&self, appid: u32) -> (String, Option<String>) {
        let mut display_name = format!("App {appid}");
        let mut installdir = None;

        // Try to get info from PICS first as it's authoritative
        let mut request = CMsgClientPICSProductInfoRequest::new();
        request
            .apps
            .push(cmsg_client_picsproduct_info_request::AppInfo {
                appid: Some(appid),
                ..Default::default()
            });

        if let Some(conn) = self.connection.as_ref() {
            let res: Result<CMsgClientPICSProductInfoResponse, _> = conn.job(request).await;
            if let Ok(response) = res {
                if let Some(app) = response.apps.iter().find(|entry| entry.appid() == appid) {
                    if let Ok(raw_vdf) = String::from_utf8(app.buffer().to_vec()) {
                        if let Ok(parsed) = parse_appinfo(&raw_vdf) {
                            let common = parsed
                                .appinfo
                                .as_ref()
                                .and_then(|a| a.common.as_ref())
                                .or(parsed.common.as_ref());
                            if let Some(common) = common {
                                if let Some(name) = &common.name {
                                    display_name = name.clone();
                                }
                                if let Some(dir) = &common.installdir {
                                    installdir = Some(dir.clone());
                                }
                            }
                        }
                    }
                }
            }
        }

        if installdir.is_none() || display_name.starts_with("App ") {
            if let Ok(games) = load_library_cache().await {
                if let Some(game) = games.iter().find(|g| g.app_id == appid) {
                    if display_name.starts_with("App ") && !game.name.is_empty() && !game.name.starts_with("App ") {
                        display_name = game.name.clone();
                    }
                }
            }
        }

        (display_name, installdir)
    }

    async fn resolve_install_game_name(&self, appid: u32) -> String {
        self.resolve_install_game_info(appid).await.0
    }

    /// Write a Steam `appmanifest_<appid>.acf` that the desktop client recognises as a
    /// complete, up-to-date install — so opening the Steam launcher does **not** treat
    /// the game as out-of-date and re-download over the files we just wrote.
    ///
    /// The two fields that resolve that clash are `buildid` (Steam compares it against
    /// the latest build in PICS; a missing/zero value reads as "update available") and
    /// `StateFlags`. When `fully_installed` is false the manifest is written in the
    /// "update required" state (4 → fully installed, 2 → update required) so an install
    /// in progress is registered with Steam rather than appearing as a fresh download.
    pub fn write_appmanifest(
        path: &Path,
        appid: u32,
        game_name: &str,
        installdir: &str,
        installed_depots: Vec<(u32, u64, u64)>,
        buildid: Option<&str>,
        fully_installed: bool,
    ) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed creating {}", parent.display()))?;
        }

        let game_name = game_name.replace('"', "");
        let buildid = buildid.unwrap_or("0");
        let size_on_disk: u64 = installed_depots.iter().map(|(_, _, size)| *size).sum();
        // 4 = StateFullyInstalled, 2 = StateUpdateRequired.
        let state_flags = if fully_installed { 4 } else { 2 };
        let bytes_have = if fully_installed { size_on_disk } else { 0 };
        let last_updated = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut content = format!(
            "\"AppState\"\n{{\n\
             \t\"appid\"\t\t\"{appid}\"\n\
             \t\"Universe\"\t\t\"1\"\n\
             \t\"name\"\t\t\"{game_name}\"\n\
             \t\"StateFlags\"\t\t\"{state_flags}\"\n\
             \t\"installdir\"\t\t\"{installdir}\"\n\
             \t\"LastUpdated\"\t\t\"{last_updated}\"\n\
             \t\"SizeOnDisk\"\t\t\"{size_on_disk}\"\n\
             \t\"StagingSize\"\t\t\"0\"\n\
             \t\"buildid\"\t\t\"{buildid}\"\n\
             \t\"LastOwner\"\t\t\"0\"\n\
             \t\"UpdateResult\"\t\t\"0\"\n\
             \t\"BytesToDownload\"\t\t\"{size_on_disk}\"\n\
             \t\"BytesDownloaded\"\t\t\"{bytes_have}\"\n\
             \t\"BytesToStage\"\t\t\"{size_on_disk}\"\n\
             \t\"BytesStaged\"\t\t\"{bytes_have}\"\n\
             \t\"AutoUpdateBehavior\"\t\t\"0\"\n\
             \t\"AllowOtherDownloadsWhileRunning\"\t\t\"0\"\n\
             \t\"ScheduledAutoUpdate\"\t\t\"0\"\n"
        );

        if !installed_depots.is_empty() {
            content.push_str("\t\"InstalledDepots\"\n\t{\n");
            for (depot_id, manifest_id, size) in installed_depots {
                content.push_str(&format!(
                    "\t\t\"{depot_id}\"\n\t\t{{\n\t\t\t\"manifest\"\t\t\"{manifest_id}\"\n\t\t\t\"size\"\t\t\"{size}\"\n\t\t}}\n"
                ));
            }
            content.push_str("\t}\n");
        }

        content.push_str("}\n");

        std::fs::write(path, content)
            .with_context(|| format!("failed writing {}", path.display()))?;
        Ok(())
    }

    /// Mark a DLC as installed and enabled in the base game's appmanifest:
    ///
    /// 1. Add the DLC's downloaded depots to `InstalledDepots`, tagged with `dlcappid`
    ///    (how Steam records DLC content as present).
    /// 2. Remove the DLC's appid from every `DisabledDLC` list (how Steam records the
    ///    DLC as enabled vs. disabled).
    ///
    /// Existing depot entries are left untouched, so re-installs are idempotent.
    pub fn enable_dlc_in_appmanifest(
        base_manifest: &Path,
        dlc_appid: u32,
        depots: &[(u32, u64, u64)],
    ) -> Result<()> {
        let mut content = std::fs::read_to_string(base_manifest)
            .with_context(|| format!("failed reading {}", base_manifest.display()))?;
        let mut changed = false;

        // 1. Ensure the DLC's depots are recorded with their dlcappid.
        let mut entries = String::new();
        for (depot_id, manifest_id, size) in depots {
            if content.contains(&format!("\"{depot_id}\"")) {
                continue; // already recorded
            }
            entries.push_str(&format!(
                "\t\t\"{depot_id}\"\n\t\t{{\n\t\t\t\"manifest\"\t\t\"{manifest_id}\"\n\t\t\t\"size\"\t\t\"{size}\"\n\t\t\t\"dlcappid\"\t\t\"{dlc_appid}\"\n\t\t}}\n"
            ));
        }
        if !entries.is_empty() {
            if let Some(pos) = content.find("\"InstalledDepots\"") {
                let rel = content[pos..].find('{').ok_or_else(|| {
                    anyhow!("malformed InstalledDepots block in {}", base_manifest.display())
                })?;
                let insert_at = pos + rel + 1;
                content.insert_str(insert_at, &format!("\n{entries}"));
            } else {
                let last = content
                    .rfind('}')
                    .ok_or_else(|| anyhow!("malformed appmanifest {}", base_manifest.display()))?;
                content.insert_str(last, &format!("\t\"InstalledDepots\"\n\t{{\n{entries}\t}}\n"));
            }
            changed = true;
        }

        // 2. Remove the DLC appid from any "DisabledDLC" lists (enable it).
        let dlc_str = dlc_appid.to_string();
        let re = regex::Regex::new(r#""DisabledDLC"(\s*)"([^"]*)""#)
            .expect("valid DisabledDLC regex");
        let updated = re.replace_all(&content, |caps: &regex::Captures| {
            let kept: Vec<&str> = caps[2]
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty() && *s != dlc_str)
                .collect();
            format!("\"DisabledDLC\"{}\"{}\"", &caps[1], kept.join(","))
        });
        if updated != content {
            content = updated.into_owned();
            changed = true;
        }

        if changed {
            std::fs::write(base_manifest, &content)
                .with_context(|| format!("failed writing {}", base_manifest.display()))?;
            tracing::info!("Enabled DLC {dlc_appid} in {}", base_manifest.display());
        } else {
            tracing::info!("DLC {dlc_appid} already enabled in {}", base_manifest.display());
        }
        Ok(())
    }

    /// Enable or disable an owned DLC by editing the base game's appmanifest
    /// `DisabledDLC` lists. Returns the base game's appid.
    ///
    /// Note: the running Steam client is authoritative for DLC enable/disable and may
    /// rewrite this state on launch; this edits the on-disk manifest only.
    pub async fn set_dlc_enabled(&self, dlc_appid: u32, enabled: bool) -> Result<u32> {
        let base_appid = self.resolve_dlc_parent(dlc_appid).await.ok_or_else(|| {
            anyhow!("app {dlc_appid} is not a DLC, or its base game could not be determined")
        })?;

        let manifest = self.appmanifest_path(base_appid).await?;
        if !manifest.exists() {
            bail!("base game (app {base_appid}) for DLC {dlc_appid} is not installed");
        }

        let content = std::fs::read_to_string(&manifest)
            .with_context(|| format!("failed reading {}", manifest.display()))?;
        let updated = apply_dlc_disabled(&content, dlc_appid, !enabled);
        if updated != content {
            std::fs::write(&manifest, &updated)
                .with_context(|| format!("failed writing {}", manifest.display()))?;
        }
        tracing::info!(
            "{} DLC {dlc_appid} in {}",
            if enabled { "Enabled" } else { "Disabled" },
            manifest.display()
        );
        Ok(base_appid)
    }

    /// Resolve the ownership / install / enable status of each DLC of a base game.
    ///
    /// `owned` comes from the account (an app ownership ticket is issued only for
    /// licensed apps). `installed` and `disabled` are read from the base game's
    /// appmanifest — if the base game isn't installed, both are `false` for every DLC.
    pub async fn dlc_states(&self, base_appid: u32, dlc_ids: &[u32]) -> Result<Vec<DlcState>> {
        // Local install/enable state lives in the base game's appmanifest.
        let (installed, disabled) = match self.appmanifest_path(base_appid).await {
            Ok(path) if path.exists() => {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("failed reading {}", path.display()))?;
                (
                    parse_installed_dlc_appids(&content),
                    parse_disabled_dlc_appids(&content),
                )
            }
            _ => (HashSet::new(), HashSet::new()),
        };

        let mut out = Vec::with_capacity(dlc_ids.len());
        for &dlc_id in dlc_ids {
            // An ownership ticket is only issued for apps the account is licensed for.
            let owned = self.get_app_ticket(dlc_id).await.is_ok();
            out.push(DlcState {
                app_id: dlc_id,
                owned,
                installed: installed.contains(&dlc_id),
                disabled: disabled.contains(&dlc_id),
            });
        }
        Ok(out)
    }

    /// Whether the desktop Steam client appears to be running.
    ///
    /// The running client caches each game's appmanifest at startup, so changes we
    /// make on disk (e.g. enabling a DLC) aren't visible to games until Steam
    /// re-reads them — which it does on restart.
    #[cfg(target_os = "windows")]
    pub fn steam_is_running() -> bool {
        read_steam_registry("SteamPID")
            .and_then(|v| {
                let v = v.trim();
                v.strip_prefix("0x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| v.parse::<u32>().ok())
            })
            .map(|pid| pid != 0)
            .unwrap_or(false)
    }

    #[cfg(not(target_os = "windows"))]
    pub fn steam_is_running() -> bool {
        false
    }

    /// Ask the desktop Steam client to shut down, and wait for it to fully exit.
    /// Windows only. Editing appmanifests is only reliable while Steam is stopped,
    /// because Steam flushes its in-memory app state to disk on exit.
    #[cfg(target_os = "windows")]
    pub fn shutdown_steam() -> Result<()> {
        if !SteamClient::steam_is_running() {
            return Ok(());
        }
        let exe = steam_exe_path().context("could not locate steam.exe to stop Steam")?;
        Command::new(&exe)
            .arg("-shutdown")
            .spawn()
            .context("failed to signal Steam shutdown")?;
        for _ in 0..60 {
            if !SteamClient::steam_is_running() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        bail!("Steam did not shut down within 30s")
    }

    /// Start the desktop Steam client. Windows only.
    #[cfg(target_os = "windows")]
    pub fn start_steam() -> Result<()> {
        let exe = steam_exe_path().context("could not locate steam.exe to start Steam")?;
        Command::new(&exe)
            .spawn()
            .context("failed to start Steam")?;
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    pub fn shutdown_steam() -> Result<()> {
        bail!("automatic Steam control is only supported on Windows")
    }

    #[cfg(not(target_os = "windows"))]
    pub fn start_steam() -> Result<()> {
        bail!("automatic Steam control is only supported on Windows")
    }

    /// Stop a game previously launched by `aurelia play`. Looks up the launch
    /// record Aurelia wrote (PID, and for a per-game Proton/Wine launch the
    /// WINEPREFIX) and terminates the process tree, then clears the record.
    ///
    /// Returns the resolved record on success. Fails if Aurelia has no record of
    /// the game running — e.g. it was started directly through Steam rather than
    /// `aurelia play`.
    pub fn stop_game(app_id: u32) -> Result<crate::running::RunningGame> {
        let record = crate::running::load(app_id).ok_or_else(|| {
            anyhow!("app {app_id} is not running (no launch was recorded by Aurelia)")
        })?;

        // A Proton/Wine game runs as wine processes inside its WINEPREFIX; killing
        // the recorded runner PID alone can leave them behind. Sweep the per-game
        // prefix too when we recorded one (never the shared master prefix).
        #[cfg(unix)]
        if let Some(prefix) = record.wineprefix.as_deref() {
            Self::kill_wine_processes_in_prefix(prefix);
        }

        kill_process_tree(record.pid);
        crate::running::clear(app_id);
        Ok(record)
    }

    /// Terminate every wine process running inside `wineprefix` (identified by the
    /// prefix path appearing in the process environment). Used to stop a
    /// Proton/Wine game whose processes outlive the runner we spawned. Only call
    /// this for a per-game prefix — the shared master prefix also hosts Steam.
    #[cfg(unix)]
    pub fn kill_wine_processes_in_prefix(wineprefix: &Path) {
        let prefix_str = wineprefix.to_string_lossy().to_string();
        let Ok(proc_dir) = std::fs::read_dir("/proc") else {
            return;
        };

        for entry in proc_dir.flatten() {
            let pid_path = entry.path();
            let Some(pid_str) = pid_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !pid_str.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }

            let environ = match std::fs::read(pid_path.join("environ")) {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(_) => continue,
            };
            if !environ.contains(&prefix_str) {
                continue;
            }

            if let Ok(pid) = pid_str.parse::<i32>() {
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
            }
        }
    }

    pub fn kill_steam_in_prefix(wineprefix: &Path) {
        #[cfg(unix)]
        {
            let prefix_str = wineprefix.to_string_lossy().to_string();
            let Ok(proc_dir) = std::fs::read_dir("/proc") else {
                return;
            };

            for entry in proc_dir.flatten() {
                let pid_path = entry.path();
                let Some(pid_str) = pid_path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !pid_str.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }

                let cmdline = match std::fs::read(pid_path.join("cmdline")) {
                    Ok(b) => String::from_utf8_lossy(&b).replace('\0', " "),
                    Err(_) => continue,
                };
                // Kill steam.exe and steamwebhelper.exe processes in this prefix
                if !cmdline.to_lowercase().contains("steam.exe")
                    && !cmdline.to_lowercase().contains("steamwebhelper.exe")
                {
                    continue;
                }

                let environ = match std::fs::read(pid_path.join("environ")) {
                    Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                    Err(_) => continue,
                };
                if !environ.contains(&prefix_str) {
                    continue;
                }

                if let Ok(pid) = pid_str.parse::<i32>() {
                    unsafe {
                        libc::kill(pid, libc::SIGTERM);
                    }
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = wineprefix;
        }
    }

    /// Scans /proc to find a wine process running steam.exe inside the given WINEPREFIX.
    pub fn is_steam_running_in_prefix(wineprefix: &Path) -> bool {
        #[cfg(unix)]
        {
            let prefix_str = wineprefix.to_string_lossy().to_string();

            let Ok(proc_dir) = std::fs::read_dir("/proc") else {
                return false;
            };

            for entry in proc_dir.flatten() {
                let pid_path = entry.path();

                // Only look at numeric PID directories
                if !pid_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.chars().all(|c| c.is_ascii_digit()))
                    .unwrap_or(false)
                {
                    continue;
                }

                // Must have steam.exe in cmdline
                let cmdline = match std::fs::read(pid_path.join("cmdline")) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let cmdline_str = String::from_utf8_lossy(&cmdline).replace('\0', " ");
                if !cmdline_str.to_lowercase().contains("steam.exe") {
                    continue;
                }

                // Must have our WINEPREFIX in its environment
                let environ = match std::fs::read(pid_path.join("environ")) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let environ_str = String::from_utf8_lossy(&environ);
                if environ_str.contains(&prefix_str) {
                    return true;
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = wineprefix;
        }

        false
    }

    /// Writes a steam.cfg into the Steam directory that minimises UI on startup.
    pub fn write_headless_steam_cfg(steam_dir: &Path) {
        let cfg_path = steam_dir.join("steam.cfg");
        // Only write if not already present to avoid overwriting user config
        if cfg_path.exists() {
            return;
        }
        let content = "\
BootStrapperForceSelfUpdate=disable
SteamDefaultDialog=Friends
NoSavePersonalInfo=1
";
        let _ = std::fs::write(&cfg_path, content);
    }

    /// The single canonical entry point for launching a game process.
    /// This function orchestrates the launch via a staged pipeline and the appropriate runner.
    /// Bypassing this for production launches is strictly forbidden.
    /// Launch a Windows game's executable directly, with no Proton/Wine layer.
    /// Used on Windows hosts (and when `--windows` is forced), where the game's
    /// native `.exe` runs without a compatibility layer.
    pub(crate) async fn spawn_windows_native(
        &self,
        app: &LibraryGame,
        launch_info: &LaunchInfo,
        user_config: Option<&crate::models::UserAppConfig>,
    ) -> Result<std::process::Child> {
        let install_dir = if let Some(p) = &app.install_path {
            let p = PathBuf::from(p);
            if p.exists() {
                p
            } else {
                self.install_root_for_app(app.app_id).await?
            }
        } else {
            self.install_root_for_app(app.app_id).await?
        };

        // Steam VDF stores Windows paths with backslashes; normalize for the host separator.
        let exe_relative = launch_info.executable.replace('\\', "/");
        let executable = install_dir.join(&exe_relative);
        let mut args = split_args(&launch_info.arguments);

        if let Some(config) = user_config {
            if !config.launch_options.trim().is_empty() {
                args.extend(split_args(&config.launch_options));
            }
        }

        let game_working_dir: PathBuf = launch_info
            .workingdir
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|wd| install_dir.join(wd.replace('\\', "/")))
            .or_else(|| executable.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| install_dir.clone());

        // Standard Steam identity fallback so the game can resolve its app id.
        let app_id_path = game_working_dir.join("steam_appid.txt");
        std::fs::write(&app_id_path, app.app_id.to_string()).unwrap_or_default();

        let mut cmd = Command::new(&executable);
        cmd.args(&args);
        cmd.current_dir(&game_working_dir);
        cmd.env("SteamAppId", app.app_id.to_string());

        if let Some(config) = user_config {
            for (key, val) in &config.env_variables {
                cmd.env(key, val);
            }
        }

        tracing::info!(
            "Launching game (Native Windows): {:?} with args {:?}",
            executable,
            args
        );
        cmd.spawn()
            .with_context(|| format!("failed to spawn windows game {}", executable.display()))
    }

    pub(crate) async fn spawn_game_process(
        &self,
        app: &LibraryGame,
        launch_info: &LaunchInfo,
        proton_path: Option<&str>,
        launcher_config: &crate::config::LauncherConfig,
        user_config: Option<&crate::models::UserAppConfig>,
    ) -> Result<std::process::Child> {
        use crate::launch::pipeline::{LaunchPipeline, PipelineContext};
        use crate::infra::logging::{LaunchSession, EventLogger};

        let mut ctx = PipelineContext::new(app.app_id);
        ctx.app = Some(app.clone());
        ctx.launch_info = Some(launch_info.clone());
        ctx.launcher_config = Some(launcher_config.clone());
        ctx.user_config = user_config.cloned();
        ctx.proton_path = proton_path.map(|s| s.to_string());

        if let Ok(config_dir) = crate::config::config_dir() {
            let session = LaunchSession::new(&config_dir.join("logs"));
            if let Ok(logger) = EventLogger::new(&session) {
                ctx.session = Some(session);
                ctx.logger = Some(logger);
            }
        }

        let pipeline = LaunchPipeline::with_default_stages();
        pipeline.run(&mut ctx).await
            .map_err(|e| anyhow!(e))?;

        ctx.child.ok_or_else(|| anyhow!("Pipeline finished without spawning a process"))
    }

    /// Internal legacy ad-hoc launch path.
    /// TODO: Remove once NativeRunner is implemented. (Ref: issue #1)
    pub async fn internal_legacy_launch_adhoc(
        &self,
        app: &LibraryGame,
        launch_info: &LaunchInfo,
        _proton_path: Option<&str>,
        _launcher_config: &crate::config::LauncherConfig,
        user_config: Option<&crate::models::UserAppConfig>,
    ) -> Result<std::process::Child> {
        let install_dir = if let Some(p) = &app.install_path {
            let p = PathBuf::from(p);
            if p.exists() {
                p
            } else {
                self.install_root_for_app(app.app_id).await?
            }
        } else {
            self.install_root_for_app(app.app_id).await?
        };

        // Steam VDF stores Windows paths with backslashes; normalize for Linux
        let exe_relative = launch_info.executable.replace('\\', "/");
        let executable = install_dir.join(&exe_relative);
        let mut args = split_args(&launch_info.arguments);

        if let Some(config) = user_config {
            if !config.launch_options.trim().is_empty() {
                let custom_args = split_args(&config.launch_options);
                args.extend(custom_args);
            }
        }

        // Standard Steam identity fallback: steam_appid.txt
        let app_id_str = app.app_id.to_string();
        // Resolve working directory:
        // 1. Use VDF-specified workingdir if present (normalized from backslashes)
        // 2. Fall back to executable's parent
        // 3. Fall back to install_dir
        let game_working_dir: PathBuf = launch_info.workingdir
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|wd| install_dir.join(wd.replace('\\', "/")))
            .or_else(|| executable.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| install_dir.clone());

        match launch_info.target {
            LaunchTarget::NativeLinux => {
                let app_id_path = game_working_dir.join("steam_appid.txt");
                std::fs::write(&app_id_path, &app_id_str).unwrap_or_default();

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(metadata) = std::fs::metadata(&executable) {
                        let mut perms = metadata.permissions();
                        perms.set_mode(0o755);
                        let _ = std::fs::set_permissions(&executable, perms);
                    }
                }

                let mut cmd = Command::new(&executable);
                cmd.args(&args);
                cmd.current_dir(&install_dir);

                let bin_dir = executable.parent().unwrap_or_else(|| Path::new("."));
                let existing_ld = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
                let existing_path = std::env::var("PATH").unwrap_or_default();

                cmd.env("LD_LIBRARY_PATH", format!("{}:{}", bin_dir.display(), existing_ld));
                cmd.env("PATH", format!("{}:{}", bin_dir.display(), existing_path));
                cmd.env("SteamAppId", app.app_id.to_string());

                if let Some(config) = user_config {
                    for (key, val) in &config.env_variables {
                        cmd.env(key, val);
                    }
                }

                tracing::info!("Launching game (Native): {:?} with args {:?}", executable, args);
                cmd.spawn().context("failed to spawn native linux game")
            }
            LaunchTarget::WindowsProton => {
                bail!("WindowsProton targets must be launched via the Pipeline and Runner abstraction. Ad-hoc bypass is prohibited.");
            }
        }
    }
}

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
    let re = regex::Regex::new(r#""dlcappid"\s*"(\d+)""#).expect("valid dlcappid regex");
    re.captures_iter(content)
        .filter_map(|caps| caps[1].parse::<u32>().ok())
        .collect()
}

/// Collect the DLC appids listed in an appmanifest's `DisabledDLC` value(s).
/// The value is a comma-separated list of appids; multiple blocks may each carry one.
fn parse_disabled_dlc_appids(content: &str) -> HashSet<u32> {
    let re = regex::Regex::new(r#""DisabledDLC"\s*"([^"]*)""#).expect("valid DisabledDLC regex");
    re.captures_iter(content)
        .flat_map(|caps| {
            caps[1]
                .split(',')
                .filter_map(|s| s.trim().parse::<u32>().ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn apply_dlc_disabled(content: &str, dlc_appid: u32, disabled: bool) -> String {
    let dlc = dlc_appid.to_string();
    let re = regex::Regex::new(r#""DisabledDLC"(\s*)"([^"]*)""#).expect("valid regex");

    if re.is_match(content) {
        return re
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

fn parse_launch_info_from_vdf(
    appid: u32,
    raw_vdf: &str,
    _prefer_proton: bool,
) -> Result<Vec<LaunchInfo>> {
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

        // HEURISTIC: DETERMINE TARGET
        let target = if let Some(os) = os_list {
            if os.contains("linux") {
                LaunchTarget::NativeLinux
            } else if os.contains("windows") {
                LaunchTarget::WindowsProton
            } else if os.contains("macos") {
                continue;
            } // Skip Mac on non-Mac
            else {
                LaunchTarget::WindowsProton
            } // Default to Windows
        } else {
            // No OS specified? Check Extension.
            if exe.ends_with(".exe") || exe.ends_with(".bat") {
                LaunchTarget::WindowsProton
            } else if exe.contains("linux") || exe.ends_with(".sh") {
                LaunchTarget::NativeLinux
            } else {
                // Default behavior
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

        let launch_options = parse_launch_info_from_vdf(10, raw, false).expect("parse launch info");
        let launch = &launch_options[0];
        assert_eq!(launch.target, LaunchTarget::NativeLinux);
        assert_eq!(launch.executable, "linux/game.sh");
        assert_eq!(launch.arguments, "-foo -bar");
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
