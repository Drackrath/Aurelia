use anyhow::{Context, Result};
use serde::Deserialize;
use std::net::SocketAddr;

const DEFAULT_CM_ENDPOINTS: &[&str] = &[
    "155.133.248.33:27017",
    "155.133.246.66:27017",
    "162.254.197.36:27017",
    "162.254.193.47:27017",
    "146.66.152.13:27017",
];

#[derive(Debug, Deserialize)]
struct CmListResponseEnvelope {
    response: CmListResponse,
}

#[derive(Debug, Deserialize)]
struct CmListResponse {
    #[serde(default)]
    serverlist: Vec<String>,
}

pub async fn get_cm_endpoints() -> Vec<SocketAddr> {
    match fetch_dynamic_cm_list().await {
        Ok(list) if !list.is_empty() => list,
        _ => fallback_cm_endpoints(),
    }
}

async fn fetch_dynamic_cm_list() -> Result<Vec<SocketAddr>> {
    let url =
        "https://api.steampowered.com/ISteamDirectory/GetCMListForConnect/v1/?cellid=0&maxcount=20";
    let payload = reqwest::get(url)
        .await
        .context("failed requesting Steam CM list")?
        .json::<CmListResponseEnvelope>()
        .await
        .context("failed decoding Steam CM list")?;

    let mut out = Vec::new();
    for entry in payload.response.serverlist {
        if let Ok(addr) = entry.parse::<SocketAddr>() {
            out.push(addr);
        }
    }

    Ok(out)
}

pub fn fallback_cm_endpoints() -> Vec<SocketAddr> {
    DEFAULT_CM_ENDPOINTS
        .iter()
        .filter_map(|raw| raw.parse::<SocketAddr>().ok())
        .collect()
}
