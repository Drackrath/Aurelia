use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use steam_vent::connection::Connection;
use steam_vent::proto::steammessages_cloud_steamclient::{
    CCloud_ClientBeginFileUpload_Request, CCloud_ClientBeginFileUpload_Response,
    CCloud_ClientCommitFileUpload_Request, CCloud_ClientCommitFileUpload_Response,
    CCloud_ClientFileDownload_Request, CCloud_ClientFileDownload_Response,
    CCloud_EnumerateUserFiles_Request, CCloud_EnumerateUserFiles_Response,
};
use steam_vent::ConnectionTrait;
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct CloudFileEntry {
    pub filename: String,
    pub timestamp: u64,
    pub size: u64,
    pub sha_hash: Option<String>,
}

pub struct CloudClient {
    connection: Connection,
    steam_id: u64,
    http: reqwest::Client,
}

impl CloudClient {
    pub fn new(connection: Connection) -> Self {
        let steam_id = u64::from(connection.steam_id());
        Self {
            connection,
            steam_id,
            http: reqwest::Client::new(),
        }
    }

    pub fn steam_id(&self) -> u64 {
        self.steam_id
    }

    pub async fn get_file_list(&self, appid: u32) -> Result<Vec<CloudFileEntry>> {
        let request = CCloud_EnumerateUserFiles_Request {
            appid: Some(appid),
            extended_details: Some(true),
            ..Default::default()
        };

        let response: CCloud_EnumerateUserFiles_Response = self
            .connection
            .service_method(request)
            .await
            .context("failed calling Cloud.EnumerateUserFiles")?;

        Ok(response
            .files
            .into_iter()
            .map(|file| CloudFileEntry {
                filename: file.filename().to_string(),
                timestamp: file.timestamp(),
                size: u64::from(file.file_size()),
                sha_hash: file.file_sha.clone(),
            })
            .collect())
    }

    pub async fn sync_down(&self, appid: u32, local_root: impl AsRef<Path>) -> Result<()> {
        let local_root = local_root.as_ref();
        tokio::fs::create_dir_all(local_root)
            .await
            .with_context(|| format!("failed to create cloud root {}", local_root.display()))?;

        let remote_files = self.get_file_list(appid).await?;
        for remote in remote_files {
            let local_path = local_root.join(&remote.filename);
            let local_mtime = file_modified_epoch_secs(&local_path).await.ok();

            let needs_download = match local_mtime {
                None => true,
                Some(ts) => remote.timestamp > ts,
            };

            if !needs_download {
                continue;
            }

            if let Some(parent) = local_path.parent() {
                tokio::fs::create_dir_all(parent).await.with_context(|| {
                    format!("failed to create parent directory {}", parent.display())
                })?;
            }

            let body = self
                .download_file(appid, &remote.filename)
                .await
                .with_context(|| {
                    format!(
                        "failed downloading cloud file '{}' for app {}",
                        remote.filename, appid
                    )
                })?;

            tokio::fs::write(&local_path, &body)
                .await
                .with_context(|| format!("failed writing {}", local_path.display()))?;
        }

        Ok(())
    }

    pub async fn sync_up(&self, appid: u32, local_root: impl AsRef<Path>) -> Result<()> {
        let local_root = local_root.as_ref();
        if !tokio::fs::try_exists(local_root).await.unwrap_or(false) {
            return Ok(());
        }

        let remote_files = self.get_file_list(appid).await?;
        let mut remote_map: HashMap<String, CloudFileEntry> = HashMap::new();
        for entry in remote_files {
            remote_map.insert(entry.filename.clone(), entry);
        }

        for entry in WalkDir::new(local_root)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_file())
        {
            let full_path = entry.path().to_path_buf();
            let relative = full_path
                .strip_prefix(local_root)
                .context("failed building relative cloud file path")?
                .to_string_lossy()
                .replace('\\', "/");

            let local_timestamp = file_modified_epoch_secs(&full_path)
                .await
                .unwrap_or_default();
            let local_size = entry.metadata()?.len();

            let should_upload = match remote_map.get(&relative) {
                None => true,
                Some(remote) => local_timestamp > remote.timestamp || local_size != remote.size,
            };

            if !should_upload {
                continue;
            }

            let data = tokio::fs::read(&full_path)
                .await
                .with_context(|| format!("failed reading {}", full_path.display()))?;
            self.upload_file(appid, &relative, local_timestamp, data)
                .await
                .with_context(|| {
                    format!(
                        "failed uploading cloud file '{}' for app {}",
                        relative, appid
                    )
                })?;
        }

        Ok(())
    }

    async fn download_file(&self, appid: u32, filename: &str) -> Result<Vec<u8>> {
        let request = CCloud_ClientFileDownload_Request {
            appid: Some(appid),
            filename: Some(filename.to_string()),
            ..Default::default()
        };

        let response: CCloud_ClientFileDownload_Response = self
            .connection
            .service_method(request)
            .await
            .context("failed calling Cloud.ClientFileDownload")?;

        let scheme = if response.use_https() {
            "https"
        } else {
            "http"
        };
        let host = response.url_host();
        let path = response.url_path();
        if host.is_empty() || path.is_empty() {
            return Err(anyhow!("ClientFileDownload returned empty URL host/path"));
        }

        let url = format!("{scheme}://{host}{path}");
        let headers = build_header_map(response.request_headers.iter().map(|h| {
            (
                h.name.as_deref().unwrap_or_default(),
                h.value.as_deref().unwrap_or_default(),
            )
        }))?;

        let response = self
            .http
            .get(url)
            .headers(headers)
            .send()
            .await
            .context("failed cloud HTTP GET")?
            .error_for_status()
            .context("cloud HTTP GET returned failure status")?;

        Ok(response
            .bytes()
            .await
            .context("failed reading cloud download body")?
            .to_vec())
    }

    async fn upload_file(
        &self,
        appid: u32,
        filename: &str,
        timestamp: u64,
        data: Vec<u8>,
    ) -> Result<()> {
        let mut sha = Sha1::new();
        sha.update(&data);
        let file_sha = sha.finalize().to_vec();

        let begin_request = CCloud_ClientBeginFileUpload_Request {
            appid: Some(appid),
            file_size: Some(u32::try_from(data.len()).context("cloud upload larger than u32")?),
            raw_file_size: Some(u32::try_from(data.len()).context("cloud upload larger than u32")?),
            file_sha: Some(file_sha.clone()),
            time_stamp: Some(timestamp),
            filename: Some(filename.to_string()),
            ..Default::default()
        };

        let begin_response: CCloud_ClientBeginFileUpload_Response = self
            .connection
            .service_method(begin_request)
            .await
            .context("failed calling Cloud.ClientBeginFileUpload")?;

        for mut block in begin_response.block_requests {
            let scheme = if block.use_https() { "https" } else { "http" };
            let host = block.url_host().to_string();
            let path = block.url_path().to_string();
            if host.is_empty() || path.is_empty() {
                return Err(anyhow!(
                    "ClientBeginFileUpload returned empty URL host/path"
                ));
            }

            let block_offset = usize::try_from(block.block_offset()).unwrap_or(0);
            let block_length = usize::try_from(block.block_length()).unwrap_or(data.len());
            let end = block_offset.saturating_add(block_length).min(data.len());
            let payload = if block.explicit_body_data.is_some() {
                block.take_explicit_body_data()
            } else if block_offset < data.len() {
                data[block_offset..end].to_vec()
            } else {
                data.clone()
            };

            let method = match block.http_method() {
                1 => reqwest::Method::PUT,
                2 => reqwest::Method::POST,
                _ => reqwest::Method::PUT,
            };
            let headers = build_header_map(block.request_headers.iter().map(|h| {
                (
                    h.name.as_deref().unwrap_or_default(),
                    h.value.as_deref().unwrap_or_default(),
                )
            }))?;

            let url = format!("{scheme}://{host}{path}");
            self.http
                .request(method, url)
                .headers(headers)
                .body(payload)
                .send()
                .await
                .context("failed cloud HTTP upload")?
                .error_for_status()
                .context("cloud HTTP upload returned failure status")?;
        }

        let commit_request = CCloud_ClientCommitFileUpload_Request {
            transfer_succeeded: Some(true),
            appid: Some(appid),
            file_sha: Some(file_sha),
            filename: Some(filename.to_string()),
            ..Default::default()
        };

        let commit_response: CCloud_ClientCommitFileUpload_Response = self
            .connection
            .service_method(commit_request)
            .await
            .context("failed calling Cloud.ClientCommitFileUpload")?;

        if !commit_response.file_committed() {
            return Err(anyhow!(
                "Cloud.ClientCommitFileUpload returned file_committed=false"
            ));
        }

        Ok(())
    }
}

fn build_header_map<'a>(headers: impl Iterator<Item = (&'a str, &'a str)>) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        if name.is_empty() {
            continue;
        }

        let header_name = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid header name '{name}'"))?;
        let header_value = HeaderValue::from_str(value)
            .with_context(|| format!("invalid header value for '{name}'"))?;
        map.insert(header_name, header_value);
    }
    Ok(map)
}

async fn file_modified_epoch_secs(path: &Path) -> Result<u64> {
    let metadata = tokio::fs::metadata(path).await?;
    let modified = metadata.modified()?;
    let seconds = modified
        .duration_since(UNIX_EPOCH)
        .context("invalid file modified timestamp")?
        .as_secs();
    Ok(seconds)
}

pub fn default_cloud_root(steam_id: u64, appid: u32) -> Result<PathBuf> {
    let home = crate::config::home_dir()?;
    let account_id = steam_id as u32;
    Ok(home
        .join(".local/share/Steam/userdata")
        .join(account_id.to_string())
        .join(appid.to_string()))
}
