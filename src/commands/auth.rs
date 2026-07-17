//! `auth` command handlers.

use crate::daemon;
use crate::output;

use crate::commands::common::*;

use anyhow::{bail, Context, Result};
use aurelia::core::config::{load_session, save_session};
use aurelia::steam_client::SteamClient;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn cmd_login(
    username: Option<String>,
    password: Option<String>,
    guard: Option<String>,
    qr: bool,
    code: bool,
    openid: bool,
    web_token: Option<Option<String>>,
    health: bool,
    reconnect: bool,
    json: bool,
) -> Result<()> {
    if health {
        return cmd_login_health(json).await;
    }
    if reconnect {
        return cmd_login_reconnect(json).await;
    }
    if qr {
        return cmd_login_qr(json).await;
    }
    if openid {
        return cmd_login_openid(json).await;
    }
    if let Some(pasted) = web_token {
        return cmd_login_web_token(pasted, json).await;
    }

    // In `--json` mode the login is driven non-interactively (e.g. by Heroic): no
    // TTY prompts. Credentials come from flags/env, and a Steam Guard code is
    // requested via a `{event:"guard_required",...}` line and read back from stdin.
    if json {
        return cmd_login_json(username, password, guard).await;
    }

    let username = match username {
        Some(u) => u,
        None => prompt_line("Steam username: ")?,
    };
    let account = username.clone();
    let password = match password.or_else(|| std::env::var("AURELIA_PASSWORD").ok()) {
        Some(p) => p,
        None => rpassword::prompt_password("Steam password: ")
            .context("failed reading password")?,
    };

    let mut client = SteamClient::new()?;
    // `--code` (alias `--pin`) reads the Steam Guard code interactively from stdin
    // (handled inside `login`); otherwise we wait for mobile-app approval and only
    // prompt for a code on the retry path below.
    let attempt = client
        .login(username.clone(), password.clone(), guard.clone(), code)
        .await;

    match attempt {
        Ok(_) => {
            report_login_success(&account, json);
            Ok(())
        }
        Err(err) => {
            // If Steam asked for a Guard code and we don't have one yet, prompt and retry once.
            let needs_code = guard.is_none()
                && client.pending_confirmations().iter().any(|p| {
                    use aurelia::core::models::SteamGuardReq::{DeviceCode, EmailCode};
                    matches!(p.requirement, EmailCode { .. } | DeviceCode)
                });

            if needs_code {
                tracing::info!("Login method awaited: Steam Guard code");
                let code = prompt_line("Steam Guard code: ")?;
                client
                    .login(username, password, Some(code), false)
                    .await
                    .context("login failed after providing Steam Guard code")?;
                report_login_success(&account, json);
                Ok(())
            } else if client
                .pending_confirmations()
                .iter()
                .any(|p| matches!(p.requirement, aurelia::core::models::SteamGuardReq::DeviceConfirmation))
            {
                tracing::info!("Login method awaited: Steam Mobile app approval");
                bail!("approve this login in the Steam Mobile app, then run `aurelia login` again")
            } else {
                Err(err).context("login failed")
            }
        }
    }
}

/// Log in by scanning a QR code with the Steam Mobile app.
///
/// In `--json` mode the challenge URL is streamed as `{event:"qr_challenge",url}`
/// (re-emitted whenever Steam rotates the code) so a driver like Heroic can render
/// the QR itself; otherwise it's drawn to stderr as a terminal QR.
pub(crate) async fn cmd_login_qr(json: bool) -> Result<()> {
    let mut client = SteamClient::new()?;
    let result = if json {
        client.login_qr(emit_qr_challenge_json).await
    } else {
        client.login_qr(render_login_qr).await
    };
    let session = result.context("QR login failed")?;
    let account = session.account_name.clone().unwrap_or_default();
    report_login_success(&account, json);
    Ok(())
}

/// Verify the user's identity on the official Steam sign-in page in the browser
/// (Steam OpenID 2.0 — Valve offers no OpenID Connect / OAuth2 endpoint).
///
/// The password is only ever typed on `steamcommunity.com`; Aurelia receives a
/// signed assertion naming the SteamID64 and confirms it with Steam directly.
/// Steam issues **no session token** over OpenID, so this verifies identity but
/// cannot mint a client session — it never touches the persisted session, and
/// the output says so explicitly.
///
/// In `--json` mode the sign-in URL is streamed as `{event:"openid_challenge",url}`
/// (the driver opens/renders it); otherwise the default browser is opened.
pub(crate) async fn cmd_login_openid(json: bool) -> Result<()> {
    let steam_id = aurelia::web::openid::verify_identity_via_browser(|url| {
        if json {
            eprint_json_line(&serde_json::json!({ "event": "openid_challenge", "url": url }));
        } else {
            cli_eprintln!("\nComplete the sign-in on the official Steam page:\n  {url}\n");
            if aurelia::web::openid::open_in_browser(url) {
                cli_eprintln!("(opened in your default browser)");
            } else {
                cli_eprintln!("(could not open a browser automatically — open the link yourself)");
            }
        }
    })
    .await
    .context("browser (OpenID) sign-in failed")?;

    // Cross-check against the persisted session, if any, so a mismatch between
    // the browser account and the stored client session is surfaced.
    let matches_session = load_session()
        .await
        .ok()
        .and_then(|s| s.steam_id)
        .map(|stored| stored == steam_id);

    if json {
        print_json(&serde_json::json!({
            "openid_verified": true,
            "steam_id": steam_id,
            "matches_stored_session": matches_session,
            // Explicit: no session was created — Steam's OpenID attests identity only.
            "logged_in": false,
        }));
    } else {
        cli_println!("Steam identity verified on the official sign-in page.");
        cli_println!("SteamID64 : {steam_id}");
        match matches_session {
            Some(true) => cli_println!("Session   : matches the stored session's account"),
            Some(false) => cli_println!(
                "Session   : WARNING — this is a different account than the stored session"
            ),
            None => {}
        }
        cli_eprintln!(
            "\nNote: Steam's OpenID sign-in proves who you are, but Valve issues no client\n\
             session token over it. Commands that need a session still require\n\
             `aurelia login` or `aurelia login --qr`.\n\
             Your browser is now signed in, though — run `aurelia login --web-token` to\n\
             also enable the web commands (inventory, wallet, market listings)."
        );
    }
    Ok(())
}

/// Store a browser **web token** (the Legendary-style "paste the code" entry
/// point) so the web-surface commands work without a client login.
///
/// The token comes from `https://steamcommunity.com/chat/clientjstoken`, opened
/// in a browser signed in to Steam — e.g. straight after `login --openid`. A
/// driver with its own webview (Heroic) can capture that JSON automatically and
/// pass it as the flag's value or over stdin in `--json` mode (after a
/// `{event:"web_token_required"}` line, mirroring the Guard-code handshake).
///
/// Web-audience and short-lived (~24h): it powers inventory/wallet/market
/// listings, never CM commands, so it complements rather than replaces `login`.
/// Refuses a token for a different account than the stored session.
pub(crate) async fn cmd_login_web_token(pasted: Option<String>, json: bool) -> Result<()> {
    // Token sources, in order: the flag's value, the AURELIA_WEB_TOKEN env var
    // (drivers use it to avoid shell-quoting the JSON on the command line —
    // Windows spawn chains mangle embedded quotes), then prompt/stdin.
    let env_token = || {
        std::env::var("AURELIA_WEB_TOKEN")
            .ok()
            .filter(|v| !v.trim().is_empty())
    };
    let raw = match pasted.filter(|v| !v.trim().is_empty()).or_else(env_token) {
        Some(v) => v,
        None if json => {
            eprint_json_line(&serde_json::json!({
                "event": "web_token_required",
                "url": aurelia::web::web_token::CLIENTJSTOKEN_URL,
            }));
            read_stdin_line().await?
        }
        None => {
            cli_eprintln!(
                "\nIn the browser where you are signed in to Steam, open:\n  {}\n",
                aurelia::web::web_token::CLIENTJSTOKEN_URL
            );
            prompt_line("Paste the entire JSON line from that page: ")?
        }
    };

    // The stored session's SteamID doubles as the identity for bare opaque
    // tokens, which carry none of their own.
    let mut session = load_session().await.unwrap_or_default();
    let info = aurelia::web::web_token::parse_web_token(&raw, session.steam_id)?;
    let now = aurelia::web::web_token::now_unix();
    if info.expires_at.is_some_and(|exp| exp <= now) {
        bail!(
            "this web token already expired — reload the clientjstoken page in a signed-in \
             browser and paste the fresh JSON"
        );
    }

    // Never let a web token silently operate a different account than the one
    // the stored session belongs to.
    if let Some(stored) = session.steam_id
        && stored != info.steam_id
    {
        bail!(
            "this web token belongs to SteamID {} but the stored session is for {stored} — \
             run `aurelia logout` first if you really want to switch accounts",
            info.steam_id
        );
    }
    session.web_token = Some(info.token.clone());
    if session.steam_id.is_none() {
        session.steam_id = Some(info.steam_id);
    }
    if session.account_name.is_none() {
        session.account_name = info.account_name.clone();
    }
    let full_session = session.refresh_token.is_some();
    save_session(&session).await?;

    if json {
        print_json(&serde_json::json!({
            "web_token_saved": true,
            "steam_id": info.steam_id,
            "account": info.account_name,
            // null for opaque (non-JWT) tokens — only Steam knows their expiry.
            "expires_at": info.expires_at,
            // Web-audience token: the web commands work, CM commands do not.
            "logged_in": full_session,
        }));
    } else {
        cli_println!("Web token saved.");
        if let Some(account) = &info.account_name {
            cli_println!("Account   : {account}");
        }
        cli_println!("SteamID64 : {}", info.steam_id);
        match info.expires_at {
            Some(exp) => cli_println!(
                "Expires   : {} UTC",
                crate::commands::common::format_unix_timestamp(exp)
            ),
            None => cli_println!("Expires   : unknown (typically ~24h — Steam decides)"),
        }
        if !full_session {
            cli_eprintln!(
                "\nThis enables the web commands (inventory, wallet, market listings) only.\n\
                 Library/install/launch still need a full `aurelia login` or `aurelia login --qr`.\n\
                 When it expires, reload the clientjstoken page and re-run `aurelia login --web-token`."
            );
        }
    }
    Ok(())
}

/// How long to wait for a single Steam login attempt before giving up. The login
/// call blocks inside steam-vent while it waits for a Steam Guard code or for the
/// user to approve the login in the Steam Mobile app; this bounds that wait so a
/// `--json` driver never hangs indefinitely.
pub(crate) const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Run one `login` attempt with [`LOGIN_TIMEOUT`]. On timeout, returns a clear
/// error (so a `--json` driver gets `{ "error": ... }` rather than hanging).
pub(crate) async fn login_with_timeout(
    client: &mut SteamClient,
    username: &str,
    password: &str,
    guard: Option<String>,
) -> Result<aurelia::core::models::SessionState> {
    match tokio::time::timeout(
        LOGIN_TIMEOUT,
        client.login(username.to_string(), password.to_string(), guard, false),
    )
    .await
    {
        Ok(login_result) => login_result,
        Err(_) => bail!(
            "login timed out after {}s waiting for a Steam Guard code or Steam Mobile app approval",
            LOGIN_TIMEOUT.as_secs()
        ),
    }
}

/// Non-interactive password login for `--json` drivers (e.g. Heroic). Credentials
/// come from flags or `AURELIA_PASSWORD`. To keep the driver informed (and never
/// silent), it emits:
/// - `{event:"awaiting_confirmation"}` right away — the login may block while
///   Steam waits for the Guard code or Mobile-app approval, so the driver should
///   prompt the user to approve on their device;
/// - `{event:"guard_required",type:"email"|"device"}` if a typed Guard code is
///   needed and none was supplied (the code is then read as one line from stdin);
/// - `{event:"guard_required",type:"device_confirmation"}` for mobile-approval
///   accounts;
/// - finally `{logged_in:true,...}` or `{error:...}`.
///
/// Each login attempt is bounded by [`LOGIN_TIMEOUT`].
pub(crate) async fn cmd_login_json(
    username: Option<String>,
    password: Option<String>,
    guard: Option<String>,
) -> Result<()> {
    let username =
        username.context("--json login requires a username (-u/--username)")?;
    let password = password
        .or_else(|| std::env::var("AURELIA_PASSWORD").ok())
        .context("--json login requires a password (-p/--password or AURELIA_PASSWORD)")?;
    let account = username.clone();

    let mut client = SteamClient::new()?;

    // The login call below blocks inside steam-vent while it waits for the Guard
    // code / mobile confirmation. Emit this first so the driver can immediately
    // tell the user to approve the login (otherwise it sees no output until the
    // attempt completes or times out).
    eprint_json_line(&serde_json::json!({
        "event": "awaiting_confirmation",
        "message": "Signing in — if prompted, approve this login in your Steam Mobile app."
    }));

    match login_with_timeout(&mut client, &username, &password, guard.clone()).await {
        Ok(_) => {
            report_login_success(&account, true);
            Ok(())
        }
        Err(err) => {
            use aurelia::core::models::SteamGuardReq::{DeviceCode, DeviceConfirmation, EmailCode};

            // A typed Steam Guard code is needed (email or authenticator).
            let code_kind = guard.is_none().then(|| {
                client.pending_confirmations().iter().find_map(|p| match p.requirement {
                    EmailCode { .. } => Some("email"),
                    DeviceCode => Some("device"),
                    _ => None,
                })
            }).flatten();

            if let Some(kind) = code_kind {
                eprint_json_line(&serde_json::json!({ "event": "guard_required", "type": kind }));
                let code = read_stdin_line()
                    .await
                    .context("failed reading Steam Guard code from stdin")?;
                login_with_timeout(&mut client, &username, &password, Some(code))
                    .await
                    .context("login failed after providing Steam Guard code")?;
                report_login_success(&account, true);
                Ok(())
            } else if client
                .pending_confirmations()
                .iter()
                .any(|p| matches!(p.requirement, DeviceConfirmation))
            {
                eprint_json_line(
                    &serde_json::json!({ "event": "guard_required", "type": "device_confirmation" }),
                );
                bail!("approve this login in the Steam Mobile app, then run login again")
            } else {
                Err(err).context("login failed")
            }
        }
    }
}

/// Emit a QR login challenge URL as one NDJSON line
pub(crate) fn emit_qr_challenge_json(url: &str) {
    eprint_json_line(&serde_json::json!({ "event": "qr_challenge", "url": url }));
}

/// Read a single line from stdin (used to receive a Guard code from a `--json`
/// driver). Returns the trimmed contents. Routes through [`output::read_line`] so
/// that, inside the daemon, it reads the forwarding client's stdin.
pub(crate) async fn read_stdin_line() -> Result<String> {
    output::read_line()
        .await
        .context("failed reading stdin")
        .map(|s| s.trim().to_string())
}

/// Render a Steam login challenge URL as a scannable QR code on stderr, with the
/// raw URL as a fallback. Diagnostics go to stderr so stdout stays clean.
pub(crate) fn render_login_qr(url: &str) {
    match qrcode::QrCode::new(url.as_bytes()) {
        Ok(code) => {
            let rendered = code
                .render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .build();
            cli_eprintln!("\nScan this QR code with the Steam Mobile app:\n{rendered}");
        }
        Err(e) => cli_eprintln!("\n(could not render QR code: {e})"),
    }
    cli_eprintln!("Or open this link in the Steam Mobile app:\n  {url}\n");
}

pub(crate) fn report_login_success(account: &str, json: bool) {
    if json {
        print_json(&serde_json::json!({ "logged_in": true, "account": account }));
    } else {
        cli_println!("Login successful.");
    }
}

pub(crate) async fn cmd_logout(json: bool) -> Result<()> {
    let mut client = restored_client().await?;
    client.logout().await?;
    if json {
        print_json(&serde_json::json!({ "logged_out": true }));
    } else {
        cli_println!("Logged out.");
    }
    Ok(())
}

/// `login --health`: report whether a session is authenticated, without logging in.
/// When a daemon is in use this reflects its shared session (no new logon); standalone
/// it does a one-off live restore check.
pub(crate) async fn cmd_login_health(json: bool) -> Result<()> {
    let via_daemon = daemon::in_daemon();
    let status = if via_daemon {
        daemon::session_status().await
    } else {
        let client = restored_client().await?;
        let session = load_session().await.ok();
        daemon::SessionStatus {
            authenticated: client.is_authenticated(),
            account: session.as_ref().and_then(|s| s.account_name.clone()),
            // Fall back to the persisted SteamID so a web-token-only session
            // (no live connection) still reports who is signed in.
            steam_id: client
                .steam_id()
                .or(session.as_ref().and_then(|s| s.steam_id)),
            web_token: session
                .as_ref()
                .is_some_and(|s| s.web_token.as_deref().is_some_and(|t| !t.is_empty())),
        }
    };
    report_session_status(&status, via_daemon, json);
    Ok(())
}

/// `login --reconnect`: tear down and re-establish the daemon's shared session from
/// the stored token (for use after the live connection dropped).
pub(crate) async fn cmd_login_reconnect(json: bool) -> Result<()> {
    if !daemon::in_daemon() {
        bail!(
            "--reconnect needs the session daemon, but this command is running standalone \
             (AURELIA_NO_DAEMON is set, or the daemon is unreachable). Start `aurelia daemon` first."
        );
    }
    let status = daemon::force_reconnect().await;
    report_session_status(&status, true, json);
    Ok(())
}

/// Print a [`daemon::SessionStatus`] for `--health`/`--reconnect`.
pub(crate) fn report_session_status(status: &daemon::SessionStatus, via_daemon: bool, json: bool) {
    if json {
        print_json(&serde_json::json!({
            "logged_in": status.authenticated,
            "account": status.account,
            "steam_id": status.steam_id,
            "web_token": status.web_token,
            "daemon": via_daemon,
        }));
    } else {
        cli_println!(
            "Session : {}",
            if status.authenticated { "authenticated" } else { "not logged in" }
        );
        if let Some(account) = &status.account {
            cli_println!("Account : {account}");
        }
        if let Some(steam_id) = status.steam_id {
            cli_println!("SteamID : {steam_id}");
        }
        if status.web_token {
            cli_println!("Web     : web token stored (inventory/wallet/market available)");
        }
        cli_println!(
            "Daemon  : {}",
            if via_daemon { "yes (shared session)" } else { "no (standalone)" }
        );
    }
}
