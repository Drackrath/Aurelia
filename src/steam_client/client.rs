//! `SteamClient` methods: lifecycle, connection, auth, login, account.
//!
//! Split out of `steam_client.rs` for readability; the struct, shared imports
//! and free helpers live in the parent module (in scope via `use super::*`).
use super::*;

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

    /// Build a client that **reuses an already-authenticated [`Connection`]** — no
    /// CM connect, no logon. This is how the daemon serves commands: it holds one
    /// live session and hands each request a cheap clone-backed client over the same
    /// connection (steam-vent multiplexes jobs and keeps the session alive), so the
    /// per-invocation re-logon that triggers Steam's rate limits never happens.
    pub fn from_shared(connection: Connection) -> Self {
        Self {
            connection: Some(connection),
            state: LoginState::Complete,
            connected_at: Some(Instant::now()),
            active_cm: None,
            server_list: None,
            pending_confirmations: Vec::new(),
        }
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

    /// Lightweight round-trip that confirms the shared connection is *actually*
    /// alive. steam-vent keeps a [`Connection`] usable in its API even after the
    /// underlying socket dies (its heartbeat task only logs send failures and never
    /// tears the connection down), so a dropped session is invisible until a real
    /// request fails. The daemon's liveness loop calls this to surface that
    /// proactively. Uses `GetFamilyGroupForUser` — a single cheap service method —
    /// bounded by an explicit timeout so a dead socket is detected promptly.
    pub async fn probe_alive(&self) -> Result<()> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        let mut req = CFamilyGroups_GetFamilyGroupForUser_Request::new();
        req.set_steamid(u64::from(connection.steam_id()));
        let _: CFamilyGroups_GetFamilyGroupForUser_Response =
            tokio::time::timeout(PROBE_TIMEOUT, connection.service_method(req))
                .await
                .context("liveness probe timed out")?
                .context("liveness probe failed")?;
        Ok(())
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

    pub(crate) async fn resolve_server_list(&mut self) -> Result<ServerList> {
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

    pub(crate) async fn try_enter_offline_mode(&mut self) -> Result<bool> {
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
                self.pending_confirmations = methods.iter().map(map_confirmation).collect();
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

    pub(crate) fn session_from_connection(&self, account_name: String) -> Option<SessionState> {
        let connection = self.connection.as_ref()?;
        let steam_id = u64::from(connection.steam_id());
        Some(SessionState {
            account_name: Some(account_name),
            steam_id: Some(steam_id),
            refresh_token: connection.access_token().map(ToString::to_string),
            client_instance_id: None,
        })
    }

}
