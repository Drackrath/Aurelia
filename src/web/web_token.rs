//! Steam **web access tokens** pasted from the browser.
//!
//! `https://steamcommunity.com/chat/clientjstoken`, opened in a browser that is
//! signed in to Steam (e.g. right after the `login --openid` flow), returns a
//! small JSON — `{"logged_in":true,"steamid":…,"account_name":…,"token":…}` —
//! whose `token` is a short-lived, **web-audience** JWT for that account. It
//! powers Steam's web surfaces (inventory, wallet, market listings) but is
//! *not* a client refresh token: the CM logon rejects it, so it can never
//! substitute for a full `login`. `login --web-token` stores it; the web-backed
//! commands use it when no CM session is available.
//!
//! The JWT is decoded here only to read its claims (expiry, subject) — Steam's
//! servers are the ones verifying its signature when it is used.

use anyhow::{bail, Context, Result};

/// Steam's endpoint returning the signed-in browser's web token JSON.
pub const CLIENTJSTOKEN_URL: &str = "https://steamcommunity.com/chat/clientjstoken";

/// A validated web token paste: the token plus the identity it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct WebTokenInfo {
    pub steam_id: u64,
    /// Steam login (account) name, when the paste was the full JSON.
    pub account_name: Option<String>,
    pub token: String,
    /// Unix seconds after which Steam rejects the token — known only for
    /// JWT-format tokens (from the `exp` claim). Steam also issues opaque
    /// (non-JWT) web tokens, whose expiry only Steam knows.
    pub expires_at: Option<u64>,
}

/// Parse a web-token paste. Accepted forms:
///
/// - the full `clientjstoken` JSON (the recommended paste);
/// - a `steamLoginSecure` cookie value — `steamid||token` (or `%7C%7C`);
/// - a bare token value. A JWT names its own account (`sub` claim); an opaque
///   token pasted alone carries no identity, so it binds to
///   `fallback_steam_id` (the stored session's account) and is refused when
///   there is no fallback either.
///
/// When the token is a JWT its `sub` claim is cross-checked against the
/// declared SteamID, so a mixed-up paste cannot bind a token to the wrong
/// account. Opaque tokens take their identity from the paste (or fallback).
pub fn parse_web_token(raw: &str, fallback_steam_id: Option<u64>) -> Result<WebTokenInfo> {
    let raw = raw.trim().trim_matches('"').trim();
    if raw.is_empty() {
        bail!("nothing was pasted");
    }

    let (declared_steam_id, account_name, token) = if raw.starts_with('{') {
        let json: serde_json::Value =
            serde_json::from_str(raw).context("the pasted text is not valid JSON")?;
        if json.get("logged_in").and_then(|v| v.as_bool()) != Some(true) {
            bail!(
                "the browser session is not signed in (logged_in is not true) — sign in on \
                 steamcommunity.com first, then reload the clientjstoken page"
            );
        }
        let steam_id: u64 = json
            .get("steamid")
            .and_then(|v| v.as_str())
            .context("the JSON has no steamid field")?
            .parse()
            .context("the JSON steamid did not parse")?;
        let account = json
            .get("account_name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);
        let token = json
            .get("token")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .context("the JSON has no token value")?
            .to_string();
        (Some(steam_id), account, token)
    } else if let Some((id_part, token_part)) =
        raw.split_once("||").or_else(|| raw.split_once("%7C%7C"))
    {
        // A pasted `steamLoginSecure` cookie value.
        let steam_id: u64 = id_part
            .trim()
            .parse()
            .context("the part before '||' is not a SteamID64")?;
        (Some(steam_id), None, unescape_json_slashes(token_part))
    } else {
        (None, None, unescape_json_slashes(raw))
    };

    // JWT tokens carry claims we can cross-check; opaque tokens do not.
    let (subject, expires_at) = match jwt_claims(&token) {
        Ok(claims) => {
            let subject: u64 = claims
                .get("sub")
                .and_then(|v| v.as_str())
                .context("the token has no sub (SteamID) claim")?
                .parse()
                .context("the token's sub claim is not a SteamID64")?;
            if let Some(declared) = declared_steam_id
                && declared != subject
            {
                bail!(
                    "the paste names SteamID {declared} but its token belongs to {subject} — \
                     re-fetch the clientjstoken page and paste it unmodified"
                );
            }
            (subject, claims.get("exp").and_then(|v| v.as_u64()))
        }
        Err(_) => {
            let Some(declared) = declared_steam_id.or(fallback_steam_id) else {
                bail!(
                    "this token is not in JWT format and carries no account identity (and no \
                     stored session provides one) — paste the entire clientjstoken JSON line \
                     instead"
                );
            };
            (declared, None)
        }
    };

    Ok(WebTokenInfo {
        steam_id: subject,
        account_name,
        token,
        expires_at,
    })
}

/// Check a stored token's expiry, with `leeway_secs` of slack so a token about
/// to lapse mid-command is treated as already gone. Opaque (non-JWT) tokens
/// carry no readable expiry and pass — Steam rejects them server-side when
/// they lapse.
pub fn check_stored_expiry(token: &str, now_unix: u64, leeway_secs: u64) -> Result<()> {
    let Ok(claims) = jwt_claims(token) else {
        return Ok(());
    };
    if let Some(exp) = claims.get("exp").and_then(|v| v.as_u64())
        && exp <= now_unix.saturating_add(leeway_secs)
    {
        bail!(
            "the stored web token expired {} — sign in on steamcommunity.com again and re-run \
             `aurelia login --web-token`",
            crate::steam_client::unix_to_ymd(exp as i64)
        );
    }
    Ok(())
}

/// Undo JSON string escaping of `/` (`\/`), which survives when a token value
/// is copied out of raw (unparsed) JSON.
fn unescape_json_slashes(s: &str) -> String {
    s.trim().replace("\\/", "/")
}

/// Current Unix time in seconds.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Decode a JWT's payload (claims) segment. No signature verification — Steam
/// verifies the token server-side; we only read metadata out of it.
pub(crate) fn jwt_claims(token: &str) -> Result<serde_json::Value> {
    let payload = token
        .split('.')
        .nth(1)
        .filter(|s| !s.is_empty())
        .context("the token is not a JWT (expected three dot-separated segments)")?;
    let bytes = b64url_decode(payload).context("the token payload is not base64url")?;
    serde_json::from_slice(&bytes).context("the token payload is not JSON")
}

/// Minimal base64url (RFC 4648 §5) decoder — enough for JWT segments, avoiding
/// a base64 crate dependency. Accepts unpadded input; rejects anything outside
/// the alphabet.
pub(crate) fn b64url_decode(input: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in input.as_bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' => continue, // tolerate padding
            _ => bail!("invalid base64url byte 0x{byte:02x}"),
        };
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "web_token_tests.rs"]
mod tests;
