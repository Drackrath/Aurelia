//! Depot browser: list and fetch files.
use super::*;
use crate::core::error::{ErrorKind, TypedError};
use crate::library::depot_browser::{
    self as browser, DepotFileDownload, DownloadedFile, ManifestRef, RawEntry,
};
use content::{PoolError, PoolRun, host_pools};

/// Everything a manifest fetch needs.
struct ManifestAccess {
    /// App owning the depot.
    app_id: u32,
    key: [u8; 32],
    code: Option<u64>,
    manifest_id: u64,
}

/// Local IO stops; network tries on.
pub(crate) fn download_failure(wanted: &str, err: &steam_cdn::Error) -> PoolError {
    let message = format!("failed to download {wanted}: {err}");
    match install::cdn_error_kind(err) {
        Some(ErrorKind::DiskFull) => {
            PoolError::Stop(anyhow!(TypedError::new(ErrorKind::DiskFull, message)))
        }
        _ if matches!(err, steam_cdn::Error::Io(_)) => PoolError::Stop(anyhow!(message)),
        _ => PoolError::Next(anyhow!(message)),
    }
}

/// Raw fields; the type is unnameable.
macro_rules! raw_entry {
    ($file:expr) => {{
        let file = $file;
        RawEntry {
            path: file.full_path(),
            flags: file.flags(),
            link_target: file.linktarget(),
            size: file.size(),
            sha_content: file.sha_content(),
            chunks: file.chunks().len(),
        }
    }};
}

/// One manifest entry with its chunks.
#[derive(Debug, Clone, Default)]
pub(crate) struct ListedFile {
    pub raw: RawEntry,
    /// `(sha, offset, size)` of each chunk.
    pub chunks: Vec<(Vec<u8>, u64, u32)>,
}

impl SteamClient {
    /// Resolve owning app and manifest id.
    async fn manifest_access(
        &self,
        app_id: u32,
        depot_id: u32,
        manifest: &ManifestRef,
    ) -> Result<ManifestAccess> {
        let buffer = self
            .pics_buffer(app_id, "failed requesting appinfo for depot files")
            .await?;
        let resolved = depots::resolve_depot(
            self.require_connection()?,
            app_id,
            find_vdf_in_pics(&buffer)?,
            depot_id,
        )
        .await?;
        let manifest_id =
            browser::pick_manifest(&depots::manifest_infos(&resolved.rows), depot_id, manifest)?;
        self.access_for(resolved.source_app(depot_id), depot_id, manifest_id)
            .await
    }

    /// Depot key and request code.
    async fn access_for(
        &self,
        app_id: u32,
        depot_id: u32,
        manifest_id: u64,
    ) -> Result<ManifestAccess> {
        let key = self
            .get_depot_key(app_id, depot_id)
            .await
            .with_context(|| format!("no access to depot {depot_id} (is it owned?)"))?;
        let key: [u8; 32] = key
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("depot {depot_id} key has {} bytes, expected 32", key.len()))?;
        let code = match self
            .get_manifest_request_code(app_id, depot_id, manifest_id)
            .await
        {
            Ok(code) => Some(code),
            Err(e) => {
                tracing::warn!("No manifest request code for depot {depot_id}: {e:#}");
                None
            }
        };
        Ok(ManifestAccess {
            app_id,
            key,
            code,
            manifest_id,
        })
    }

    /// Live content hosts for pooling.
    async fn browser_hosts(&self) -> Result<Vec<ContentHost>> {
        let cell_id = self.require_connection()?.cell_id();
        self.get_content_servers(cell_id).await
    }

    /// CDN client over one pool.
    async fn pool_client(
        &self,
        access: &ManifestAccess,
        depot_id: u32,
        pool: &[ContentHost],
    ) -> Result<steam_cdn::CDNClient> {
        let connection = self.require_connection()?;
        let servers = self
            .cdn_servers(access.app_id, depot_id, pool, connection.cell_id())
            .await?;
        Ok(steam_cdn::CDNClient::with_servers(
            Arc::new(connection.clone()),
            servers,
        ))
    }

    /// Rows for an access, sorted.
    async fn manifest_rows(
        &self,
        depot_id: u32,
        access: &ManifestAccess,
    ) -> Result<Vec<ManifestFileEntry>> {
        let listing = self.manifest_listing(depot_id, access).await?;
        Ok(browser::entries(listing.into_iter().map(|f| f.raw)))
    }

    /// Every entry of an access's manifest, chunks included.
    async fn manifest_listing(
        &self,
        depot_id: u32,
        access: &ManifestAccess,
    ) -> Result<Vec<ListedFile>> {
        let hosts = self.browser_hosts().await?;
        let mut run = PoolRun::default();
        for pool in host_pools(&hosts) {
            let outcome: std::result::Result<_, PoolError> = async {
                let cdn = self
                    .pool_client(access, depot_id, pool)
                    .await
                    .map_err(PoolError::Next)?;
                let listing = cdn
                    .get_manifest(
                        access.app_id,
                        depot_id,
                        access.manifest_id,
                        access.code,
                        Some(access.key),
                    )
                    .await
                    .map_err(|e| PoolError::Next(anyhow!("failed to get manifest: {e}")))?;
                Ok(listing
                    .files()
                    .iter()
                    .map(|f| ListedFile {
                        raw: raw_entry!(f),
                        chunks: f
                            .chunks()
                            .iter()
                            .map(|c| (c.sha(), c.offset(), c.original_size()))
                            .collect(),
                    })
                    .collect())
            }
            .await;
            if let Some(done) = run.step(outcome) {
                return done;
            }
        }
        Err(run.exhausted())
    }

    /// Entry rows of one depot manifest.
    pub async fn depot_manifest_files(
        &self,
        app_id: u32,
        depot_id: u32,
        manifest: &ManifestRef,
    ) -> Result<(u64, Vec<ManifestFileEntry>)> {
        let access = self.manifest_access(app_id, depot_id, manifest).await?;
        let rows = self.manifest_rows(depot_id, &access).await?;
        Ok((access.manifest_id, rows))
    }

    /// Rows for a source app's manifest.
    pub async fn fetch_manifest_files(
        &self,
        source_app: u32,
        depot_id: u32,
        manifest_id: u64,
    ) -> Result<Vec<ManifestFileEntry>> {
        let access = self.access_for(source_app, depot_id, manifest_id).await?;
        self.manifest_rows(depot_id, &access).await
    }

    /// Every entry of a source app's manifest, chunks included.
    pub(crate) async fn fetch_manifest_listing(
        &self,
        source_app: u32,
        depot_id: u32,
        manifest_id: u64,
    ) -> Result<Vec<ListedFile>> {
        let access = self.access_for(source_app, depot_id, manifest_id).await?;
        self.manifest_listing(depot_id, &access).await
    }

    /// Download one depot file, SHA-1 checked.
    pub async fn download_depot_file(&self, request: &DepotFileDownload) -> Result<DownloadedFile> {
        let dest = browser::download_target(&request.out_dir, &request.path)?;
        // Fail before any network traffic.
        browser::check_dest(&dest, request.force)?;
        let depot_id = request.depot_id;
        let access = self
            .manifest_access(request.app_id, depot_id, &request.manifest)
            .await?;
        let wanted = browser::normalize_request(&request.path);
        let hosts = self.browser_hosts().await?;
        let mut run = PoolRun::default();
        for pool in host_pools(&hosts) {
            let outcome: std::result::Result<u64, PoolError> = async {
                let cdn = self
                    .pool_client(&access, depot_id, pool)
                    .await
                    .map_err(PoolError::Next)?;
                let listing = cdn
                    .get_manifest(
                        access.app_id,
                        depot_id,
                        access.manifest_id,
                        access.code,
                        Some(access.key),
                    )
                    .await
                    .map_err(|e| PoolError::Next(anyhow!("failed to get manifest: {e}")))?;
                let file = listing
                    .files()
                    .iter()
                    .find(|f| f.full_path() == wanted)
                    .ok_or_else(|| {
                        PoolError::Stop(anyhow!(
                            "{wanted} is not in this manifest (paths are case-sensitive)"
                        ))
                    })?;
                let raw = raw_entry!(file);
                let kind = browser::entry_kind(raw.flags, &raw.link_target);
                browser::ensure_file(&wanted, kind).map_err(PoolError::Stop)?;

                browser::check_dest(&dest, request.force).map_err(PoolError::Stop)?;
                let part = browser::create_part(&dest).map_err(PoolError::Stop)?;
                if let Err(e) = file
                    .download(access.key, &part, false, None, None, None)
                    .await
                {
                    let _ = std::fs::remove_file(&part);
                    return Err(download_failure(&wanted, &e));
                }
                let sha1 = hex::encode(&raw.sha_content);
                let check = browser::PlaceCheck {
                    sha1: &sha1,
                    size: raw.size,
                    force: request.force,
                };
                browser::place(&part, &dest, check).map_err(|e| {
                    // Corrupt data: another pool.
                    if e.downcast_ref::<browser::ChecksumMismatch>().is_some() {
                        PoolError::Next(e)
                    } else {
                        PoolError::Stop(e)
                    }
                })?;
                Ok(raw.size)
            }
            .await;
            if let Some(done) = run.step(outcome) {
                return Ok(DownloadedFile {
                    manifest_id: access.manifest_id,
                    path: dest,
                    size: done?,
                });
            }
        }
        Err(run.exhausted())
    }
}

#[cfg(test)]
mod tests;
