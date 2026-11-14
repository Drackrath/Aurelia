//! SDR network config (`GetSDRConfig`).
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Anonymous Web API endpoint.
const URL: &str = "https://api.steampowered.com/ISteamApps/GetSDRConfig/v1/";
/// Refetch after this long.
const TTL: Duration = Duration::from_secs(3600);
/// Comma-separated STUN servers.
pub const STUN_ENV: &str = "AURELIA_SDR_STUN";

/// `k_nSteamNetworkingConfig_P2P_Transport_ICE_Enable_*` bits.
pub mod ice {
    pub const PRIVATE: i32 = 2;
    pub const PUBLIC: i32 = 4;
}

/// The parts we use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SdrConfig {
    #[serde(default)]
    pub revision: u64,
    /// Country (or `default`) to policy.
    #[serde(default)]
    pub p2p_share_ip: HashMap<String, i64>,
}

impl SdrConfig {
    pub fn parse(json: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct Envelope {
            #[serde(default = "yes")]
            success: bool,
            #[serde(flatten)]
            config: SdrConfig,
        }
        fn yes() -> bool {
            true
        }
        let envelope: Envelope =
            serde_json::from_str(json).context("SDR config is not valid JSON")?;
        if !envelope.success {
            bail!("Steam refused the SDR config");
        }
        Ok(envelope.config)
    }

    /// Policy for `country`, else default.
    pub fn share_ip(&self, country: &str) -> Option<i64> {
        let country = country.trim().to_ascii_lowercase();
        self.p2p_share_ip
            .get(&country)
            .or_else(|| self.p2p_share_ip.get("default"))
            .copied()
    }

    /// Positive policy allows public (inferred).
    pub fn ice_enable(&self, country: &str) -> i32 {
        match self.share_ip(country) {
            Some(v) if v > 0 => ice::PRIVATE | ice::PUBLIC,
            _ => ice::PRIVATE,
        }
    }
}

/// STUN list from the environment.
pub fn stun_servers() -> String {
    let raw = std::env::var(STUN_ENV).unwrap_or_default();
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

type Cache = tokio::sync::Mutex<HashMap<u32, (Instant, SdrConfig)>>;
static CACHE: LazyLock<Cache> = LazyLock::new(Cache::default);

/// Cached config, fetched hourly at most.
pub async fn sdr_config(app_id: u32) -> Result<SdrConfig> {
    let mut cache = CACHE.lock().await;
    if let Some((at, config)) = cache.get(&app_id)
        && at.elapsed() < TTL
    {
        return Ok(config.clone());
    }
    let config = fetch(app_id).await?;
    cache.insert(app_id, (Instant::now(), config.clone()));
    Ok(config)
}

async fn fetch(app_id: u32) -> Result<SdrConfig> {
    let client = crate::core::net::steam_web_client()?;
    let request = client.get(URL).query(&[("appid", app_id)]);
    let response = crate::core::net::send_with_retry(&client, request).await?;
    let status = response.status();
    if !status.is_success() {
        bail!("GetSDRConfig returned HTTP {}", status.as_u16());
    }
    let text = response.text().await.context("GetSDRConfig body")?;
    SdrConfig::parse(&text)
}

#[cfg(test)]
#[path = "sdr_config_tests.rs"]
mod tests;
