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
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_supported_schemes() {
        for url in [
            "http://host:8080",
            "http://user:pass@host:8080",
            "https://proxy.example:3128",
            "socks5://127.0.0.1:1080",
            "socks5h://127.0.0.1:1080",
        ] {
            assert!(validate_proxy_url(url).is_ok(), "should accept {url}");
        }
    }

    #[test]
    fn validate_rejects_garbage() {
        assert!(validate_proxy_url("not a url").is_err());
    }
}
