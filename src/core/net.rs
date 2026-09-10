//! Network proxy configuration.
//!
//! Aurelia reaches the network over HTTP(S) from many places: the Steam
//! Community/store/market web endpoints, depot content downloads (steam-cdn), and
//! GitHub/Codeberg release lookups for the Proton/plugin managers. All of these build
//! `reqwest` clients, and `reqwest` honours the conventional proxy environment
//! variables (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`) unless a client
//! explicitly opts out — none of ours do.
//!
//! Rather than thread a proxy through every `reqwest::Client::builder()` call site
//! (several of which live in vendored crates where we don't construct the client),
//! [`install_proxy_env`] translates the persisted [`ProxyConfig`] into those
//! environment variables once, at process startup, before any client is built. This
//! makes a single configured proxy apply uniformly across the whole process. An
//! explicit proxy env var already set by the user always wins, matching the usual
//! proxy convention.
//!
//! Scope: this covers HTTP(S) traffic only. The Steam CM binary/WebSocket transport
//! (steam-vent) is a separate connection that Aurelia does not route through the proxy.

use crate::core::config::ProxyConfig;
use crate::core::error::{ErrorKind, TypedError};
use std::time::Duration;

/// The env vars reqwest consults to pick a proxy. We set all three so one configured
/// proxy applies to both http and https requests regardless of which variable a given
/// reqwest version keys on for a particular request.
const PROXY_VARS: [&str; 3] = ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"];

/// Whether any proxy-selecting env var is already present (in upper- or lower-case
/// form). When so, we defer entirely to the user's environment and touch nothing.
fn env_proxy_already_set() -> bool {
    PROXY_VARS.iter().any(|name| {
        std::env::var_os(name).is_some() || std::env::var_os(name.to_ascii_lowercase()).is_some()
    })
}

/// Translate `config` into the standard proxy environment variables so every `reqwest`
/// client in the process (including those built inside vendored crates) routes through
/// the configured proxy. A no-op when no proxy URL is configured.
///
/// # Threading
/// This mutates the process environment and MUST be called before the async runtime or
/// any worker threads are spawned — i.e. while the process is still single-threaded —
/// because [`std::env::set_var`] is not sound to call concurrently with other threads
/// that may be reading the environment.
pub fn install_proxy_env(config: &ProxyConfig) {
    let Some(url) = config.url.as_deref().map(str::trim).filter(|u| !u.is_empty()) else {
        return;
    };

    // An explicit proxy env var from the user takes precedence over Aurelia's config.
    if !env_proxy_already_set() {
        for name in PROXY_VARS {
            // SAFETY: `install_proxy_env` is documented to run from `main` before the
            // Tokio runtime and worker threads start, so the process is single-threaded
            // here and no other thread can be reading the environment concurrently.
            unsafe { std::env::set_var(name, url) };
        }
    }

    if let Some(no_proxy) = config.no_proxy.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        if std::env::var_os("NO_PROXY").is_none() && std::env::var_os("no_proxy").is_none() {
            // SAFETY: as above — single-threaded startup.
            unsafe { std::env::set_var("NO_PROXY", no_proxy) };
        }
    }
}

/// Validate a proxy URL the way `reqwest` will interpret it, so the CLI can reject a
/// bad value up front rather than silently failing every later request. Accepts the
/// `http`, `https`, and `socks5`/`socks5h` schemes reqwest understands.
pub fn validate_proxy_url(url: &str) -> anyhow::Result<()> {
    reqwest::Proxy::all(url)
        .map(|_| ())
        .map_err(|err| anyhow::anyhow!("invalid proxy URL `{url}`: {err}"))
}

#[cfg(test)]
#[path = "net_tests.rs"]
mod tests;

/// Attempts per request, including the first.
const HTTP_ATTEMPTS: u32 = 3;
/// Base backoff for transient failures.
const RETRY_BACKOFF_BASE: Duration = Duration::from_secs(1);
/// Longest `Retry-After` we wait for inline.
const RETRY_AFTER_CAP: Duration = Duration::from_secs(30);

/// What a response asks of us.
enum Verdict {
    Done,
    RateLimited(Option<Duration>),
    Transient,
}

impl Verdict {
    fn of(resp: &reqwest::Response) -> Self {
        let status = resp.status().as_u16();
        let headers = resp.headers();
        match status {
            429 => Self::RateLimited(retry_after(headers)),
            403 if header_str(headers, "x-ratelimit-remaining") == Some("0") => {
                Self::RateLimited(ratelimit_reset(headers))
            }
            502..=504 => Self::Transient,
            _ => Self::Done,
        }
    }
}

fn header_str<'a>(headers: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim)
}

/// Integer-seconds `Retry-After`; HTTP-dates are unused here.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    header_str(headers, "retry-after")
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// GitHub's epoch reset header, relative to now.
fn ratelimit_reset(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let reset = header_str(headers, "x-ratelimit-reset")?.parse::<u64>().ok()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(Duration::from_secs(reset.saturating_sub(now).max(1)))
}

/// Exponential backoff with sub-second jitter.
fn backoff(attempt: u32) -> Duration {
    let jitter_ms = u64::from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
            % 250,
    );
    RETRY_BACKOFF_BASE * 2u32.saturating_pow(attempt.saturating_sub(1))
        + Duration::from_millis(jitter_ms)
}

fn rate_limited(host: &str, status: reqwest::StatusCode, retry_after: Option<Duration>) -> TypedError {
    TypedError::new(
        ErrorKind::RateLimited,
        format!(
            "{host} is rate-limiting requests (HTTP {}); wait before retrying",
            status.as_u16()
        ),
    )
    .with_retry_after(retry_after)
}

/// Send a request, retrying transient failures.
///
/// A 429 is retried only for a short `Retry-After`;
/// otherwise it becomes a typed `RateLimited` error.
pub async fn send_with_retry(
    client: &reqwest::Client,
    request: reqwest::RequestBuilder,
) -> anyhow::Result<reqwest::Response> {
    use anyhow::Context;
    let request = request.build().context("failed to build HTTP request")?;
    let host = request.url().host_str().unwrap_or("?").to_string();
    for attempt in 1..=HTTP_ATTEMPTS {
        let last = attempt == HTTP_ATTEMPTS;
        // Streaming bodies can't be cloned: single shot.
        let Some(req) = request.try_clone() else {
            return client
                .execute(request)
                .await
                .with_context(|| format!("request to {host} failed"));
        };
        match client.execute(req).await {
            Ok(resp) => match Verdict::of(&resp) {
                Verdict::Done => return Ok(resp),
                Verdict::RateLimited(retry) => match retry {
                    Some(wait) if wait <= RETRY_AFTER_CAP && !last => {
                        tracing::warn!("{host} rate-limited; retrying in {}s", wait.as_secs());
                        tokio::time::sleep(wait).await;
                    }
                    _ => return Err(rate_limited(&host, resp.status(), retry).into()),
                },
                Verdict::Transient => {
                    if last {
                        return Ok(resp);
                    }
                    let wait = backoff(attempt);
                    tracing::warn!(
                        "{host} returned HTTP {}; retrying in {:.1}s",
                        resp.status().as_u16(),
                        wait.as_secs_f32()
                    );
                    tokio::time::sleep(wait).await;
                }
            },
            Err(e) if !last && (e.is_timeout() || e.is_connect()) => {
                let wait = backoff(attempt);
                tracing::warn!(
                    "request to {host} failed ({e}); retrying in {:.1}s",
                    wait.as_secs_f32()
                );
                tokio::time::sleep(wait).await;
            }
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!("request to {host} failed")));
            }
        }
    }
    unreachable!("loop returns on the last attempt")
}

/// Aurelia-UA client with `timeout`.
pub fn http_client(timeout: Duration) -> anyhow::Result<reqwest::Client> {
    use anyhow::Context;
    reqwest::Client::builder()
        .user_agent("aurelia")
        .timeout(timeout)
        .build()
        .context("failed to build HTTP client")
}

/// "aurelia" UA, 20-second timeout.
pub fn steam_web_client() -> anyhow::Result<reqwest::Client> {
    http_client(Duration::from_secs(20))
}
