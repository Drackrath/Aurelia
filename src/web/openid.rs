//! Browser-based Steam identity verification (OpenID 2.0).
//!
//! Steam's official web sign-in for third parties is **OpenID 2.0** — Valve
//! offers no OAuth2/OpenID **Connect** endpoint. The flow proves *who* the user
//! is: they sign in on `steamcommunity.com` itself (their password never
//! touches Aurelia) and Steam redirects the browser back to a localhost
//! callback with a signed assertion naming their SteamID64, which we then
//! confirm with Steam directly (`check_authentication`).
//!
//! Deliberate limitation, for callers: Steam issues **no session or refresh
//! token** over OpenID, so this cannot replace a full client login — Valve
//! provides no browser-redirect flow that can mint CM-session credentials for
//! third-party clients. Aurelia therefore surfaces it as a secure *identity
//! check* (`login --openid`); a full session still comes from the
//! password/Steam Guard or QR flows.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashSet;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The one endpoint Steam serves OpenID 2.0 on. Assertions claiming any other
/// `op_endpoint` are rejected outright.
pub const STEAM_OPENID_ENDPOINT: &str = "https://steamcommunity.com/openid/login";

/// Query parameter carrying our per-attempt random nonce inside `return_to`.
/// Steam signs `return_to`, so the callback provably belongs to this attempt.
const NONCE_PARAM: &str = "aurelia_nonce";

/// How long to wait for the user to finish signing in on the Steam page.
const BROWSER_WAIT: Duration = Duration::from_secs(300);

/// Smallest SteamID64 of an individual account (universe 1, type 1).
const MIN_INDIVIDUAL_STEAMID: u64 = 76561197960265728;

/// Run the whole browser sign-in flow and return the verified SteamID64.
///
/// Binds a localhost-only callback listener, invokes `on_url` once with the
/// official Steam sign-in URL (the caller renders it and/or opens a browser),
/// then waits — bounded by [`BROWSER_WAIT`] — for Steam to redirect back.
/// The assertion is accepted only when every local check passes **and** Steam
/// itself confirms the signature via `check_authentication`.
pub async fn verify_identity_via_browser<F>(mut on_url: F) -> Result<u64>
where
    F: FnMut(&str),
{
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("failed to bind the localhost callback listener")?;
    let port = listener
        .local_addr()
        .context("failed to read the callback listener address")?
        .port();

    let nonce = format!("{:032x}", rand::random::<u128>());
    let return_to = format!("http://127.0.0.1:{port}/callback?{NONCE_PARAM}={nonce}");
    let realm = format!("http://127.0.0.1:{port}");
    let auth_url = build_auth_url(&return_to, &realm)?;
    on_url(auth_url.as_str());
    tracing::info!("Login method awaited: sign-in on the official Steam page in the browser");

    let http = reqwest::Client::builder()
        .user_agent("aurelia")
        .timeout(Duration::from_secs(20))
        .build()
        .context("failed to build HTTP client")?;

    tokio::time::timeout(BROWSER_WAIT, async {
        loop {
            let (mut stream, _) = listener
                .accept()
                .await
                .context("callback listener failed")?;
            let head = read_request_head(&mut stream).await.unwrap_or_default();
            let Some(target) = request_target(&head) else {
                respond(&mut stream, "400 Bad Request", FAILURE_PAGE).await;
                continue;
            };
            // Browsers also ask for /favicon.ico etc. — ignore anything that
            // isn't the OpenID callback and keep waiting.
            if target != "/callback" && !target.starts_with("/callback?") {
                respond(&mut stream, "404 Not Found", FAILURE_PAGE).await;
                continue;
            }

            let params = parse_query_pairs(target)?;
            match validate_assertion(&params, &return_to) {
                Ok(steam_id) => match check_authentication(&http, &params).await {
                    Ok(()) => {
                        respond(&mut stream, "200 OK", SUCCESS_PAGE).await;
                        return Ok(steam_id);
                    }
                    Err(err) => {
                        respond(&mut stream, "403 Forbidden", FAILURE_PAGE).await;
                        return Err(err);
                    }
                },
                Err(err) => {
                    respond(&mut stream, "400 Bad Request", FAILURE_PAGE).await;
                    return Err(err).context("the Steam OpenID assertion failed validation");
                }
            }
        }
    })
    .await
    .map_err(|_| {
        anyhow!(
            "browser sign-in timed out after {}s without a completed Steam sign-in",
            BROWSER_WAIT.as_secs()
        )
    })?
}

/// Best-effort: open `url` in the user's default browser. Returns whether a
/// launcher process could be spawned (the URL is always also printed).
pub fn open_in_browser(url: &str) -> bool {
    #[cfg(target_os = "windows")]
    let spawned = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let spawned = std::process::Command::new("xdg-open").arg(url).spawn();
    spawned.is_ok()
}

/// Build the `checkid_setup` URL the browser is sent to on `steamcommunity.com`.
fn build_auth_url(return_to: &str, realm: &str) -> Result<reqwest::Url> {
    let mut url =
        reqwest::Url::parse(STEAM_OPENID_ENDPOINT).context("invalid Steam OpenID endpoint")?;
    url.query_pairs_mut()
        .append_pair("openid.ns", "http://specs.openid.net/auth/2.0")
        .append_pair("openid.mode", "checkid_setup")
        .append_pair("openid.return_to", return_to)
        .append_pair("openid.realm", realm)
        .append_pair(
            "openid.identity",
            "http://specs.openid.net/auth/2.0/identifier_select",
        )
        .append_pair(
            "openid.claimed_id",
            "http://specs.openid.net/auth/2.0/identifier_select",
        );
    Ok(url)
}

/// Local checks on the redirect parameters before asking Steam to confirm the
/// signature: correct mode, the official `op_endpoint`, a `return_to` that is
/// byte-identical to ours (whose signed random nonce ties the assertion to this
/// attempt), signature coverage of every security-relevant field, and a
/// `claimed_id` naming an individual SteamID64. Returns that SteamID64.
pub(crate) fn validate_assertion(
    params: &[(String, String)],
    expected_return_to: &str,
) -> Result<u64> {
    let get = |key: &str| {
        params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };

    match get("openid.mode") {
        Some("id_res") => {}
        Some("cancel") => bail!("the sign-in was cancelled on the Steam page"),
        other => bail!("unexpected openid.mode {other:?} in the callback"),
    }
    if get("openid.op_endpoint") != Some(STEAM_OPENID_ENDPOINT) {
        bail!("the assertion does not name the official Steam OpenID endpoint");
    }
    if get("openid.return_to") != Some(expected_return_to) {
        bail!("openid.return_to does not match this login attempt (possible replay)");
    }

    // Steam must have signed every field we rely on; an unsigned field could
    // have been tampered with in transit through the browser.
    let signed: HashSet<&str> = get("openid.signed").unwrap_or("").split(',').collect();
    for field in [
        "claimed_id",
        "identity",
        "return_to",
        "response_nonce",
        "assoc_handle",
        "op_endpoint",
    ] {
        if !signed.contains(field) {
            bail!("Steam's signature does not cover the {field} field");
        }
    }

    let claimed = get("openid.claimed_id").context("missing openid.claimed_id")?;
    if get("openid.identity") != Some(claimed) {
        bail!("openid.identity and openid.claimed_id disagree");
    }
    steam_id_from_claimed_id(claimed)
}

/// Parse the SteamID64 out of a `https://steamcommunity.com/openid/id/<id>`
/// claimed identity URL, requiring an individual-account id.
pub(crate) fn steam_id_from_claimed_id(claimed: &str) -> Result<u64> {
    let id = claimed
        .strip_prefix("https://steamcommunity.com/openid/id/")
        .or_else(|| claimed.strip_prefix("http://steamcommunity.com/openid/id/"))
        .context("claimed_id is not a steamcommunity.com identity URL")?;
    if id.len() != 17 || !id.bytes().all(|b| b.is_ascii_digit()) {
        bail!("claimed_id does not contain a well-formed SteamID64");
    }
    let steam_id: u64 = id.parse().context("claimed_id SteamID64 did not parse")?;
    if steam_id < MIN_INDIVIDUAL_STEAMID {
        bail!("claimed_id is not an individual-account SteamID64");
    }
    Ok(steam_id)
}

/// Ask Steam to confirm the assertion's signature (OpenID 2.0 stateless
/// verification): every received `openid.*` field is POSTed back with the mode
/// swapped to `check_authentication`, and Steam answers `is_valid:true` exactly
/// once per `response_nonce` — which also rules out replays.
async fn check_authentication(http: &reqwest::Client, params: &[(String, String)]) -> Result<()> {
    // Form-urlencode by hand (via the url crate's form serializer behind
    // `query_pairs_mut`) — this build of reqwest carries no `form` feature.
    let mut encoder = reqwest::Url::parse(STEAM_OPENID_ENDPOINT)
        .context("invalid Steam OpenID endpoint")?;
    {
        let mut pairs = encoder.query_pairs_mut();
        pairs.clear();
        for (k, v) in params.iter().filter(|(k, _)| k.starts_with("openid.")) {
            if k == "openid.mode" {
                pairs.append_pair(k, "check_authentication");
            } else {
                pairs.append_pair(k, v);
            }
        }
    }
    let form_body = encoder.query().unwrap_or_default().to_string();

    let body = http
        .post(STEAM_OPENID_ENDPOINT)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(form_body)
        .send()
        .await
        .context("could not reach Steam to verify the sign-in assertion")?
        .error_for_status()
        .context("Steam rejected the verification request")?
        .text()
        .await
        .context("failed reading Steam's verification response")?;

    if !verification_says_valid(&body) {
        bail!("Steam did not confirm the sign-in assertion (is_valid != true)");
    }
    Ok(())
}

/// Parse Steam's key-value verification response for `is_valid:true`.
pub(crate) fn verification_says_valid(body: &str) -> bool {
    body.lines().any(|line| line.trim() == "is_valid:true")
}

/// Decode the query string of the callback request target into raw pairs.
pub(crate) fn parse_query_pairs(target: &str) -> Result<Vec<(String, String)>> {
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}"))
        .context("the callback request URL did not parse")?;
    Ok(url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect())
}

/// Read a request's head (request line + headers). GET requests carry no body,
/// so reading up to the blank line is enough; capped defensively at 16 KiB.
async fn read_request_head(stream: &mut TcpStream) -> Result<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await.context("callback read failed")?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// The request target of a `GET <target> HTTP/x.y` request line, if that is
/// what `head` holds.
pub(crate) fn request_target(head: &str) -> Option<&str> {
    let mut parts = head.lines().next()?.split_whitespace();
    if parts.next()? != "GET" {
        return None;
    }
    parts.next()
}

/// Send a minimal HTML response and close the connection. Best-effort — the
/// flow's outcome never depends on the browser actually receiving this.
async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

const SUCCESS_PAGE: &str = "<!DOCTYPE html><html><head><title>Aurelia</title></head><body style=\"font-family:sans-serif;text-align:center;margin-top:4em\"><h2>Steam sign-in verified</h2><p>You can close this tab and return to the terminal.</p><p style=\"max-width:36em;margin:2em auto;color:#555\">Optional: to also enable Aurelia's web features (inventory, wallet, market listings), open <a href=\"https://steamcommunity.com/chat/clientjstoken\">steamcommunity.com/chat/clientjstoken</a> in this browser, copy the JSON shown, and run <code>aurelia login --web-token</code>.</p></body></html>";

const FAILURE_PAGE: &str = "<!DOCTYPE html><html><head><title>Aurelia</title></head><body style=\"font-family:sans-serif;text-align:center;margin-top:4em\"><h2>Steam sign-in not verified</h2><p>Return to the terminal for details.</p></body></html>";

#[cfg(test)]
#[path = "openid_tests.rs"]
mod tests;
