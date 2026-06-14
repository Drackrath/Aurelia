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

use anyhow::{bail, Context, Result};

/// CDN base for economy item icons (`icon_url` values are relative to this).
pub const ECON_IMAGE_BASE: &str =
    "https://community.cloudflare.steamstatic.com/economy/image/";

/// An authenticated Steam web session: a `reqwest` client plus the cookies Steam's
/// web endpoints require.
#[derive(Clone)]
pub struct WebSession {
    http: reqwest::Client,
    steam_id: u64,
    /// The `sessionid` CSRF token. Steam only checks that this cookie matches the
    /// `sessionid` form field on POSTs, so a client-generated value is accepted.
    sessionid: String,
    /// Pre-rendered `Cookie:` header value.
    cookie: String,
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
            cookie,
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
        let resp = self
            .http
            .get(url)
            .header(reqwest::header::COOKIE, &self.cookie)
            .send()
            .await
            .with_context(|| format!("request to {url} failed"))?;
        read_checked(resp).await
    }
}

/// Map HTTP status to an error or the body text.
async fn read_checked(resp: reqwest::Response) -> Result<String> {
    let status = resp.status();
    if status.as_u16() == 429 {
        bail!(
            "Steam is rate-limiting market requests (HTTP 429). Wait a few minutes before \
             trying again — repeated requests during a block extend it."
        );
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        bail!(
            "the Steam web session was rejected (HTTP {}). Run `aurelia login --reconnect`, \
             or `aurelia login` if you are not signed in.",
            status.as_u16()
        );
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
            bail!(
                "this account cannot use the Steam Community Market yet. Steam requires the \
                 Steam Guard Mobile Authenticator enabled for 15+ days (and no recent new-device \
                 holds) before market and wallet features become available."
            );
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
    resp.text().await.context("failed reading the response body")
}

/// A random 24-hex-character `sessionid` token.
fn random_sessionid() -> String {
    use rand::RngExt;
    let mut rng = rand::rng();
    (0..24).map(|_| char::from_digit(rng.random_range(0..16), 16).unwrap()).collect()
}
