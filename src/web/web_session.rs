//! Minimal Steam **web** session.
//!
//! The Steam Community Market, inventory, and wallet are HTTP surfaces on
//! `steamcommunity.com` — not the CM binary protocol `steam-vent` speaks — so they
//! authenticate with browser cookies, chiefly `steamLoginSecure`. This module holds
//! those cookies plus a `reqwest` client, built from a short-lived **web access
//! token** minted off the CM session (see `SteamClient::web_session`).
//!
//! Cookies are hand-set on a `Cookie:` header rather than via `reqwest`'s cookie
//! store, so no extra crate feature is needed for the two values we manage.

use crate::core::error::{ErrorKind, TypedError};
use anyhow::{Context, Result, bail};

/// CDN base for economy item icons (`icon_url` values are relative to this).
pub const ECON_IMAGE_BASE: &str = "https://community.cloudflare.steamstatic.com/economy/image/";

/// An authenticated Steam web session: a `reqwest` client plus the cookies Steam's
/// web endpoints require.
#[derive(Clone)]
pub struct WebSession {
    http: reqwest::Client,
    steam_id: u64,
    /// The `sessionid` CSRF token. Steam only checks that this cookie matches the
    /// `sessionid` form field on POSTs, so a client-generated value is accepted.
    sessionid: String,
    /// Pre-rendered `Cookie:` header value; grows with server cookies.
    cookie: std::sync::Arc<std::sync::Mutex<crate::core::secret::Secret>>,
}

impl WebSession {
    /// Build a session from a freshly minted web access token. `country` (e.g. the
    /// account's GeoIP country code) sets `steamCountry`, which some market endpoints
    /// use to resolve wallet currency.
    pub fn new(steam_id: u64, web_access_token: &str, country: Option<&str>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("aurelia")
            .timeout(std::time::Duration::from_secs(20))
            // Don't follow redirects: an unauthenticated request bounces to the login
            // page, so a 30x is a definitive "session rejected" signal rather than a
            // confusing redirect loop or a login-page body parsed as data.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("failed to build HTTP client")?;

        // steamLoginSecure = urlencode("<steamid64>||<access_token>"); the only
        // character needing encoding is `|` → %7C (the JWT itself is URL-safe).
        let login_secure = format!("{steam_id}%7C%7C{web_access_token}");
        let sessionid = random_sessionid();
        let mut cookie = format!("steamLoginSecure={login_secure}; sessionid={sessionid}");
        if let Some(cc) = country.filter(|c| !c.is_empty()) {
            cookie.push_str("; steamCountry=");
            cookie.push_str(cc);
        }

        Ok(Self {
            http,
            steam_id,
            sessionid,
            cookie: std::sync::Arc::new(std::sync::Mutex::new(cookie.into())),
        })
    }

    /// SteamID64 this session is authenticated as.
    pub fn steam_id(&self) -> u64 {
        self.steam_id
    }

    /// The `sessionid` CSRF token (echoed in POST bodies for write actions).
    pub fn sessionid(&self) -> &str {
        &self.sessionid
    }

    /// Perform an authenticated GET and return the response body text. Maps Steam's
    /// common failure modes (rate limiting, an expired/invalid session) to clear,
    /// actionable errors.
    pub async fn get_text(&self, url: &str) -> Result<String> {
        let mut resp = self.send_get(url).await?;
        if let Some(check) = eligibility_redirect(&resp) {
            self.pass_eligibility_check(&check).await?;
            // Retry once with new cookies.
            resp = self.send_get(url).await?;
        }
        read_checked(resp).await
    }

    /// GET with the current cookies.
    async fn send_get(&self, url: &str) -> Result<reqwest::Response> {
        let cookie = self.cookie_header();
        let request = self
            .http
            .get(url)
            .header(reqwest::header::COOKIE, cookie.expose());
        crate::core::net::send_with_retry(&self.http, request).await
    }

    /// Snapshot of the cookie header.
    fn cookie_header(&self) -> crate::core::secret::Secret {
        let guard = self.cookie.lock().unwrap_or_else(|e| e.into_inner());
        guard.expose().into()
    }

    /// Visit the check once; keep its cookies.
    async fn pass_eligibility_check(&self, check_url: &str) -> Result<()> {
        tracing::debug!("following Steam market eligibility check");
        let resp = self.send_get(check_url).await?;
        if redirect_location(&resp).is_some_and(|l| l.contains("/login")) {
            bail!(
                "the Steam web session was rejected (redirected to login). Run \
                 `aurelia login --reconnect`."
            );
        }
        let set: Vec<(String, String)> = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(parse_set_cookie)
            .collect();
        if set.is_empty() {
            return Err(ineligible_error());
        }
        let mut guard = self.cookie.lock().unwrap_or_else(|e| e.into_inner());
        let merged = merge_cookies(guard.expose(), &set);
        *guard = merged.into();
        Ok(())
    }
}

/// A redirect's `Location` header, if any.
fn redirect_location(resp: &reqwest::Response) -> Option<String> {
    if !resp.status().is_redirection() {
        return None;
    }
    resp.headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// Absolute eligibility-check URL, if redirected there.
fn eligibility_redirect(resp: &reqwest::Response) -> Option<String> {
    let location = redirect_location(resp)?;
    if !location.contains("eligibilitycheck") {
        return None;
    }
    resp.url().join(&location).ok().map(|u| u.to_string())
}

/// Typed "market not available" error.
fn ineligible_error() -> anyhow::Error {
    TypedError::new(
        ErrorKind::AccessDenied,
        "this account cannot use the Steam Community Market yet. Steam requires the \
         Steam Guard Mobile Authenticator enabled for 15+ days (and no recent new-device \
         holds) before market and wallet features become available.",
    )
    .into()
}

/// Name and value of a `Set-Cookie` header.
fn parse_set_cookie(header: &str) -> Option<(String, String)> {
    let pair = header.split(';').next()?.trim();
    let (name, value) = pair.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some((name.to_string(), value.trim().to_string()))
}

/// Merge cookies; existing auth cookies win.
fn merge_cookies(existing: &str, new: &[(String, String)]) -> String {
    // Never let the server replace these.
    const KEEP: &[&str] = &["steamLoginSecure", "sessionid"];
    let mut pairs: Vec<(String, String)> = existing
        .split(';')
        .filter_map(|p| {
            let (n, v) = p.trim().split_once('=')?;
            Some((n.to_string(), v.to_string()))
        })
        .collect();
    for (name, value) in new {
        if KEEP.contains(&name.as_str()) {
            continue;
        }
        match pairs.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = value.clone(),
            None => pairs.push((name.clone(), value.clone())),
        }
    }
    pairs
        .iter()
        .map(|(n, v)| format!("{n}={v}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Map HTTP status to an error or the body text.
async fn read_checked(resp: reqwest::Response) -> Result<String> {
    let status = resp.status();
    if status.as_u16() == 429 {
        return Err(TypedError::new(
            ErrorKind::RateLimited,
            "Steam is rate-limiting market requests (HTTP 429). Wait a few minutes before \
             trying again — repeated requests during a block extend it.",
        )
        .into());
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(TypedError::new(
            ErrorKind::AuthRequired,
            format!(
                "the Steam web session was rejected (HTTP {}). Run `aurelia login --reconnect`, \
                 or `aurelia login` if you are not signed in.",
                status.as_u16()
            ),
        )
        .into());
    }
    if status.is_redirection() {
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("(unknown)");
        // Steam redirects authenticated-but-ineligible accounts to an eligibility
        // check; an actually-rejected session goes to the login page. Distinguish them.
        if location.contains("eligibilitycheck") {
            return Err(ineligible_error());
        }
        if location.contains("/login") {
            bail!(
                "the Steam web session was rejected (redirected to login). Run \
                 `aurelia login --reconnect`."
            );
        }
        bail!(
            "the request was redirected unexpectedly (HTTP {} → {location}).",
            status.as_u16()
        );
    }
    resp.text()
        .await
        .context("failed reading the response body")
}

/// A random 24-hex-character `sessionid` token.
fn random_sessionid() -> String {
    use rand::RngExt;
    let mut rng = rand::rng();
    (0..24)
        .map(|_| char::from_digit(rng.random_range(0..16), 16).unwrap())
        .collect()
}

#[cfg(test)]
#[path = "web_session_tests.rs"]
mod tests;
