//! `auth` command handlers.

use crate::daemon;
use crate::output;

use crate::commands::common::*;

use anyhow::{bail, Context, Result};
use aurelia::core::config::load_session;
use aurelia::steam_client::SteamClient;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn cmd_login(
    username: Option<String>,
    password: Option<String>,
    guard: Option<String>,
    qr: bool,
    code: bool,
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
        daemon::SessionStatus {
            authenticated: client.is_authenticated(),
            account: load_session().await.ok().and_then(|s| s.account_name),
            steam_id: client.steam_id(),
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
        cli_println!(
            "Daemon  : {}",
            if via_daemon { "yes (shared session)" } else { "no (standalone)" }
        );
    }
}
