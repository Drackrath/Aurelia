//! CM connection, logon ids and retries.

use super::*;

/// How long to wait for a single Steam CM connection/refresh-token logon before
/// giving up on that server. The initial WebSocket handshake has no timeout of
/// its own, so an unresponsive CM would otherwise block the command forever.
pub(super) const CM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Upper bound on a single liveness probe ([`SteamClient::probe_alive`]). Generous
/// enough not to false-positive when the connection is merely busy (e.g. during a
/// heavy download), short enough to detect a dead socket within one probe cycle.
pub(super) const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// How many CM servers to try before failing. `ServerList::pick_ws` rotates
/// round-robin, so each attempt hits a different server.
pub(super) const CM_CONNECT_ATTEMPTS: usize = 3;

/// Daemon-chosen logon ID; 0 means unset.
pub(super) static LOGON_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Use a stable logon ID from `key`.
///
/// For the daemon, so a respawn replaces only its own earlier session.
pub fn use_stable_logon_id(key: &str) {
    LOGON_ID.store(stable_logon_id(key), std::sync::atomic::Ordering::Relaxed);
}

/// FNV-1a of `key`, never 0.
pub fn stable_logon_id(key: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in key.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash | 1
}

/// This process's logon ID, never 0.
///
/// Steam replaces a session when another logs on with the same ID from the
/// same public IP, so each process gets its own unless the daemon set one.
pub fn logon_id() -> u32 {
    use std::sync::atomic::Ordering;
    let current = LOGON_ID.load(Ordering::Relaxed);
    if current != 0 {
        return current;
    }
    let random = rand::random::<u32>() | 1;
    match LOGON_ID.compare_exchange(0, random, Ordering::Relaxed, Ordering::Relaxed) {
        Ok(_) => random,
        Err(set) => set,
    }
}

/// Authenticate with a stored refresh token, with a per-attempt timeout and
/// automatic failover to the next CM server.
///
/// `Connection::login_with_refresh_token_and_logon_id` connects to a single WebSocket CM server and performs
/// the refresh-token logon, but it never times out — a stalled CM leaves the
/// whole command hanging (the reported symptom). Each call re-picks a server
/// round-robin, so on timeout/error we simply retry, advancing to the next one.
pub(super) async fn access_with_retry(
    server_list: &ServerList,
    account: &str,
    refresh_token: &str,
) -> Result<Connection> {
    let _ = account; // 0.6 reads it from the token
    let token = RefreshToken::new(refresh_token.to_string())
        .map_err(|e| anyhow!("stored refresh token is invalid: {e}"))?;
    let logon_id = logon_id();
    let mut last_err: Option<anyhow::Error> = None;

    for attempt in 1..=CM_CONNECT_ATTEMPTS {
        tracing::debug!(
            attempt,
            attempts = CM_CONNECT_ATTEMPTS,
            logon_id,
            "Authenticating with stored refresh token ..."
        );
        let logon = async {
            let unauthenticated =
                steam_vent::connection::UnAuthenticatedConnection::connect(server_list).await?;
            let licences = super::licence_list::subscribe(&unauthenticated);
            let connection = unauthenticated
                .login_with_refresh_token_and_logon_id(&token, logon_id)
                .await?;
            Ok::<_, ConnectionError>((connection, licences))
        };
        match tokio::time::timeout(CM_CONNECT_TIMEOUT, logon).await {
            Ok(Ok((connection, licences))) => {
                tracing::debug!("Refresh-token authentication succeeded");
                super::licence_list::watch(licences, u64::from(connection.steam_id()));
                return Ok(connection);
            }
            // Retrying these only deepens a lockout.
            Ok(Err(
                err @ ConnectionError::LoginError(
                    LoginError::RateLimited
                    | LoginError::InvalidCredentials
                    | LoginError::UnavailableAccount
                    | LoginError::InvalidSteamId,
                ),
            )) => {
                tracing::warn!("CM logon rejected: {err}; not retrying");
                return Err(anyhow::Error::new(err).context("refresh token login failed"));
            }
            Ok(Err(err)) => {
                tracing::warn!("CM connect attempt {attempt}/{CM_CONNECT_ATTEMPTS} failed: {err}");
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

    Err(last_err.unwrap_or_else(|| anyhow!("failed to connect to any Steam CM server")))
        .context("refresh token login failed")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoginState {
    Connected,
    AwaitingCredentialSession,
    AwaitingGuardConfirmation,
    AwaitingPollResult,
    AwaitingAccessTokenLogon,
    Complete,
    Offline,
}

pub(super) fn map_confirmation(method: &ConfirmationMethod) -> ConfirmationPrompt {
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
