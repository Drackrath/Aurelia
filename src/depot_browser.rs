use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use sha1::Digest;
use std::collections::HashMap;
use std::path::Path;

use steam_vent::connection::Connection;
use steam_vent::proto::steammessages_clientserver_appinfo::{
    cmsg_client_picsproduct_info_request, CMsgClientPICSProductInfoRequest,
    CMsgClientPICSProductInfoResponse,
};
use steam_vent::ConnectionTrait;

use steam_cdn::CDNClient;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct DepotInfo {
    pub depot_id: u32,
    pub name: String,
    pub max_size: u64,
    pub public_manifest_id: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ManifestFileEntry {
    pub filename: String,
    pub size: u64,
    pub sha_hash: String,
    pub chunks: usize,
}

#[derive(Debug, Deserialize)]
struct AppInfoRoot {
    #[serde(default)]
    appinfo: Option<AppInfoNode>,
    #[serde(default)]
    depots: HashMap<String, DepotNode>,
}

#[derive(Debug, Deserialize)]
struct AppInfoNode {
    #[serde(default)]
    depots: HashMap<String, DepotNode>,
}

#[derive(Debug, Deserialize)]
struct DepotNode {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    maxsize: Option<String>,
    #[serde(default)]
    manifests: Option<DepotManifests>,
}

#[derive(Debug, Deserialize)]
struct DepotManifests {
    #[serde(default)]
    public: Option<String>,
}

pub async fn fetch_depots(connection: &Connection, appid: u32) -> Result<Vec<DepotInfo>> {
    let parsed = fetch_appinfo(connection, appid).await?;
    let depots = parsed.appinfo.map(|n| n.depots).unwrap_or(parsed.depots);

    let mut out = Vec::new();
    for (depot_id, node) in depots {
        if !depot_id.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let depot_id = depot_id.parse::<u32>()?;
        let max_size = node
            .maxsize
            .as_deref()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or_default();
        let public_manifest_id = node
            .manifests
            .and_then(|m| m.public)
            .and_then(|v| v.parse::<u64>().ok());

        out.push(DepotInfo {
            depot_id,
            name: node.name.unwrap_or_else(|| format!("Depot {depot_id}")),
            max_size,
            public_manifest_id,
        });
    }

    out.sort_by_key(|d| d.depot_id);
    Ok(out)
}

pub async fn fetch_manifest_files(
    connection: &Connection,
    appid: u32,
    depot_id: u32,
    manifest_ref: &str,
) -> Result<Vec<ManifestFileEntry>> {
    let manifest_id = resolve_manifest_id(connection, appid, depot_id, manifest_ref).await?;
    let cdn_client = CDNClient::new(Arc::new(connection.clone()));
    let manifest = cdn_client
        .get_manifest(appid, depot_id, manifest_id, None, None)
        .await
        .map_err(|e| anyhow!("failed to get manifest: {e}"))?;

    let mut files = Vec::new();
    for file in manifest.files() {
        let filename = file.full_path();
        if filename.is_empty() {
            continue;
        }

        let mut hasher = sha1::Sha1::new();
        for chunk in file.chunks() {
            hasher.update(chunk.sha());
        }
        let sha_hash = hex::encode(hasher.finalize());

        files.push(ManifestFileEntry {
            filename,
            size: file.size(),
            sha_hash,
            chunks: file.chunks().len(),
        });
    }

    files.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(files)
}

pub fn download_single_file(
    connection: &Connection,
    appid: u32,
    depot_id: u32,
    manifest_ref: &str,
    file_path: &str,
    output_dir: &Path,
) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let manifest_id = runtime.block_on(resolve_manifest_id(
        connection,
        appid,
        depot_id,
        manifest_ref,
    ))?;

    let cdn_client = CDNClient::new(Arc::new(connection.clone()));
    let manifest = runtime
        .block_on(cdn_client.get_manifest(appid, depot_id, manifest_id, None, None))
        .map_err(|e| anyhow!("failed to get manifest: {e}"))?;

    let file = manifest
        .files()
        .iter()
        .find(|f| f.full_path() == file_path)
        .ok_or_else(|| anyhow!("file not found in manifest: {file_path}"))?;

    let depot_key = runtime
        .block_on(cdn_client.get_depot_decryption_key(appid, depot_id))
        .map_err(|e| anyhow!("failed to get depot key: {e}"))?
        .ok_or_else(|| anyhow!("no decryption key for depot {depot_id}"))?;

    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("failed creating {}", output_dir.display()))?;

    let full_path = output_dir.join(file.full_path());
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).context("failed creating directory for file")?;
    }

    runtime
        .block_on(file.download(depot_key, &full_path, false, None, None, None))
        .map_err(|e| anyhow!("failed to download file: {e}"))?;

    Ok(())
}

async fn resolve_manifest_id(
    connection: &Connection,
    appid: u32,
    depot_id: u32,
    manifest_ref: &str,
) -> Result<u64> {
    if let Ok(id) = manifest_ref.trim().parse::<u64>() {
        return Ok(id);
    }

    if manifest_ref.trim().is_empty() || manifest_ref.eq_ignore_ascii_case("public") {
        let depots = fetch_depots(connection, appid).await?;
        return depots
            .into_iter()
            .find(|d| d.depot_id == depot_id)
            .and_then(|d| d.public_manifest_id)
            .ok_or_else(|| anyhow!("missing public manifest for depot {depot_id}"));
    }

    bail!("unsupported manifest reference '{manifest_ref}', expected numeric id or 'public'")
}

async fn fetch_appinfo(connection: &Connection, appid: u32) -> Result<AppInfoRoot> {
    let mut request = CMsgClientPICSProductInfoRequest::new();
    request
        .apps
        .push(cmsg_client_picsproduct_info_request::AppInfo {
            appid: Some(appid),
            ..Default::default()
        });

    let response: CMsgClientPICSProductInfoResponse = connection
        .job(request)
        .await
        .context("failed requesting appinfo product info")?;

    let app = response
        .apps
        .iter()
        .find(|entry| entry.appid() == appid)
        .ok_or_else(|| anyhow!("missing appinfo payload for app {appid}"))?;

    let raw_vdf = String::from_utf8_lossy(app.buffer()).to_string();
    keyvalues_serde::from_str(&raw_vdf).context("failed parsing appinfo VDF")
}
