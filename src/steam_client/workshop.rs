//! `SteamClient` methods for the Steam Workshop: published-file metadata
//! (`PublishedFile.GetDetails`), collection expansion, subscription enumeration
//! (`ClientUCMEnumerateUserSubscribedFilesWithUpdates`) and (un)subscribe.
//!
//! Only metadata + subscription state live here; downloading a Workshop item's
//! content (its SteamPipe depot manifest) is the install pipeline's job.
//!
//! Split out of `steam_client.rs` for readability; the struct, shared imports
//! and free helpers live in the parent module (in scope via `use super::*`).
use super::*;

use crate::models::{WorkshopItem, WorkshopItemKind};
use steam_vent_proto::steammessages_clientserver_ucm::CMsgClientUCMEnumerateUserSubscribedFilesWithUpdates;
use steam_vent_proto::steammessages_publishedfile_steamclient::{
    CPublishedFile_GetDetails_Request, CPublishedFile_GetDetails_Response,
    CPublishedFile_Subscribe_Request, CPublishedFile_Subscribe_Response,
    CPublishedFile_Unsubscribe_Request, CPublishedFile_Unsubscribe_Response,
};

/// Maximum depth `expand_collections` will recurse into nested collections
/// before giving up, as a backstop in addition to the visited-set cycle guard.
const MAX_COLLECTION_DEPTH: usize = 8;

impl SteamClient {
    /// Fetch metadata for a batch of Workshop published files in a single
    /// `PublishedFile.GetDetails` call. Ids that the service returns no data for
    /// (deleted/private/invalid) are skipped with a warning rather than failing
    /// the whole batch.
    pub async fn fetch_published_file_details(&self, ids: &[u64]) -> Result<Vec<WorkshopItem>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.connection.as_ref().ok_or_else(|| anyhow!("No connection"))?;

        let mut request = CPublishedFile_GetDetails_Request::new();
        // `publishedfileids` is a `repeated fixed64` → a plain `Vec<u64>`.
        request.publishedfileids = ids.to_vec();
        // We need collection membership to classify/expand collections.
        request.set_includechildren(true);

        let response: CPublishedFile_GetDetails_Response = connection
            .service_method(request)
            .await
            .context("failed calling PublishedFile.GetDetails")?;

        let mut out = Vec::with_capacity(response.publishedfiledetails.len());
        for detail in &response.publishedfiledetails {
            // `result` is an EResult; 1 == OK. A non-OK result (or a zeroed
            // publishedfileid) means Steam had nothing for that id.
            let id = detail.publishedfileid();
            if detail.result() != 1 || id == 0 {
                tracing::warn!(
                    "PublishedFile.GetDetails returned no usable data for id {id} (result {})",
                    detail.result()
                );
                continue;
            }

            let children: Vec<u64> = detail
                .children
                .iter()
                .map(|c| c.publishedfileid())
                .filter(|&cid| cid != 0)
                .collect();

            // A collection is a Workshop entry whose payload is a list of member
            // ids. The proto exposes both a `num_children` count and the actual
            // `children` list; treat either signal as a collection.
            let kind = if !children.is_empty() || detail.num_children() > 0 {
                WorkshopItemKind::Collection
            } else {
                WorkshopItemKind::Item
            };

            out.push(WorkshopItem {
                id,
                app_id: detail.consumer_appid(),
                title: detail.title().to_string(),
                hcontent_file: detail.hcontent_file(),
                file_url: detail.file_url().to_string(),
                file_size: detail.file_size(),
                // proto field is `uint32`; widen to the model's `i64`.
                time_updated: i64::from(detail.time_updated()),
                kind,
                children,
            });
        }

        Ok(out)
    }

    /// Resolve `ids` to a flat, de-duplicated, first-seen-ordered list of leaf
    /// (non-collection) item ids, recursing into nested collections. Plain item
    /// ids pass through unchanged. Cycles are broken with a visited set and
    /// recursion is capped at [`MAX_COLLECTION_DEPTH`].
    pub async fn expand_collections(&self, ids: &[u64]) -> Result<Vec<u64>> {
        let mut result: Vec<u64> = Vec::new();
        let mut seen_leaf: HashSet<u64> = HashSet::new();
        // Ids whose details we've already resolved/queued, to avoid re-fetching
        // and to break collection cycles.
        let mut visited: HashSet<u64> = HashSet::new();

        // Work queue of (id, depth). We resolve a whole frontier per depth level
        // in one batched GetDetails call.
        let mut frontier: Vec<u64> = Vec::new();
        for &id in ids {
            if visited.insert(id) {
                frontier.push(id);
            }
        }

        let mut depth = 0usize;
        while !frontier.is_empty() {
            if depth > MAX_COLLECTION_DEPTH {
                tracing::warn!(
                    "expand_collections hit max depth {MAX_COLLECTION_DEPTH}; {} ids left unexpanded, treating as leaves",
                    frontier.len()
                );
                for id in frontier.drain(..) {
                    if seen_leaf.insert(id) {
                        result.push(id);
                    }
                }
                break;
            }

            let details = self.fetch_published_file_details(&frontier).await?;
            // Map id -> kind/children for this frontier so we preserve the
            // first-seen order of `frontier` rather than the response order.
            let mut info: HashMap<u64, (WorkshopItemKind, Vec<u64>)> = HashMap::new();
            for d in details {
                info.insert(d.id, (d.kind, d.children));
            }

            let current = std::mem::take(&mut frontier);
            for id in current {
                match info.remove(&id) {
                    Some((WorkshopItemKind::Collection, children)) => {
                        for child in children {
                            if visited.insert(child) {
                                frontier.push(child);
                            }
                        }
                    }
                    // Plain item, or an id Steam returned nothing for (skipped in
                    // fetch_published_file_details). Unknown ids are treated as
                    // leaves so the caller still sees them.
                    _ => {
                        if seen_leaf.insert(id) {
                            result.push(id);
                        }
                    }
                }
            }

            depth += 1;
        }

        Ok(result)
    }

    /// Enumerate the logged-in user's subscribed Workshop published files for
    /// `app_id`, returning their publishedfileids. Sent as a UCM *client*
    /// message (a job), not a service method.
    pub async fn fetch_subscribed_items(&self, app_id: u32) -> Result<Vec<u64>> {
        let connection = self.connection.as_ref().ok_or_else(|| anyhow!("No connection"))?;

        let mut request = CMsgClientUCMEnumerateUserSubscribedFilesWithUpdates::new();
        request.set_app_id(app_id);
        request.set_start_index(0);

        let response: steam_vent_proto::steammessages_clientserver_ucm::CMsgClientUCMEnumerateUserSubscribedFilesWithUpdatesResponse = connection
            .job(request)
            .await
            .context("failed enumerating subscribed Workshop files")?;

        if response.eresult() != 1 {
            tracing::warn!(
                "EnumerateUserSubscribedFiles for app {app_id} returned eresult {}",
                response.eresult()
            );
        }

        let ids: Vec<u64> = response
            .subscribed_files
            .iter()
            .map(|f| f.published_file_id())
            .filter(|&id| id != 0)
            .collect();
        tracing::debug!("user has {} subscribed Workshop items for app {app_id}", ids.len());
        Ok(ids)
    }

    /// Subscribe the logged-in user to a Workshop item.
    pub async fn subscribe_published_file(&self, id: u64, app_id: u32) -> Result<()> {
        let connection = self.connection.as_ref().ok_or_else(|| anyhow!("No connection"))?;

        let mut request = CPublishedFile_Subscribe_Request::new();
        request.set_publishedfileid(id);
        request.set_appid(app_id as i32);
        request.set_notify_client(true);

        let _response: CPublishedFile_Subscribe_Response = connection
            .service_method(request)
            .await
            .with_context(|| format!("failed subscribing to Workshop item {id}"))?;

        tracing::info!("subscribed to Workshop item {id} (app {app_id})");
        Ok(())
    }

    /// Unsubscribe the logged-in user from a Workshop item.
    pub async fn unsubscribe_published_file(&self, id: u64, app_id: u32) -> Result<()> {
        let connection = self.connection.as_ref().ok_or_else(|| anyhow!("No connection"))?;

        let mut request = CPublishedFile_Unsubscribe_Request::new();
        request.set_publishedfileid(id);
        request.set_appid(app_id as i32);
        request.set_notify_client(true);

        let _response: CPublishedFile_Unsubscribe_Response = connection
            .service_method(request)
            .await
            .with_context(|| format!("failed unsubscribing from Workshop item {id}"))?;

        tracing::info!("unsubscribed from Workshop item {id} (app {app_id})");
        Ok(())
    }

    /// Download a Workshop item's content into
    /// `steamapps/workshop/content/<appid>/<id>/` and register it in
    /// `appworkshop_<appid>.acf`. Returns a progress receiver, mirroring
    /// `install_game` so the CLI can drive it with the same `drive_progress`.
    ///
    /// v1 supports SteamPipe items only (those with a non-zero `hcontent_file`);
    /// legacy `file_url` UGC returns an error. Workshop content is served on the
    /// app's workshop depot, whose id equals the consumer app id.
    pub async fn install_workshop_item(
        &self,
        item: &WorkshopItem,
        shared_state: Arc<std::sync::RwLock<crate::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        if item.hcontent_file == 0 {
            bail!(
                "Workshop item {} has no SteamPipe content (legacy file_url UGC is not yet supported)",
                item.id
            );
        }

        let cfg = load_launcher_config().await?;
        let library_root = PathBuf::from(&cfg.steam_library_path);
        let app_id = item.app_id;
        let depot_id = app_id; // workshop content lives on the app's workshop depot
        let manifest_id = item.hcontent_file;
        let published_file_id = item.id;
        let time_updated = item.time_updated;
        let title = item.title.clone();

        let dest = super::workshop_manifest::workshop_content_dir(&library_root, app_id, published_file_id);
        std::fs::create_dir_all(&dest)
            .with_context(|| format!("failed creating {}", dest.display()))?;
        let manifest_path = super::workshop_manifest::workshop_manifest_path(&library_root, app_id);

        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let client = self.clone();

        tokio::spawn(async move {
            let _ = tx
                .send(DownloadProgress {
                    state: DownloadProgressState::Queued,
                    current_file: String::new(),
                    ..Default::default()
                })
                .await;

            if let Ok(mut state) = shared_state.write() {
                state.is_downloading = true;
                state.is_paused = false;
                state.app_id = app_id;
                state.app_name = format!("Workshop item {published_file_id} ({title})");
                state.downloaded_bytes = 0;
                state.total_bytes = 0;
                state.status_text = format!("Downloading Workshop item {published_file_id} ...");
            }

            // Forward the live byte counters the download updates into progress
            // messages, on a timer — same pattern as `install_game`.
            let progress_tx = tx.clone();
            let progress_state = shared_state.clone();
            let ticker = tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_millis(250));
                loop {
                    ticker.tick().await;
                    let snapshot = match progress_state.read() {
                        Ok(s) => Some((
                            s.is_downloading,
                            s.downloaded_bytes,
                            s.total_bytes,
                            s.status_text.clone(),
                            s.depot_id,
                            s.depot_downloaded_bytes,
                            s.depot_total_bytes,
                        )),
                        Err(_) => None,
                    };
                    let Some((downloading, downloaded, total, status, depot_id, depot_dl, depot_total)) =
                        snapshot
                    else {
                        break;
                    };
                    if !downloading {
                        break;
                    }
                    if progress_tx
                        .send(DownloadProgress {
                            state: DownloadProgressState::Downloading,
                            bytes_downloaded: downloaded,
                            total_bytes: total,
                            current_file: status,
                            depot_id,
                            depot_bytes_downloaded: depot_dl,
                            depot_total_bytes: depot_total,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });

            let outcome = client
                .download_depot_to(app_id, depot_id, manifest_id, &dest, shared_state.clone())
                .await;

            // Stop the ticker before emitting the terminal message.
            if let Ok(mut state) = shared_state.write() {
                state.is_downloading = false;
            }
            let _ = ticker.await;

            match outcome {
                Ok(size) => {
                    let record = super::workshop_manifest::InstalledWorkshopItem {
                        published_file_id,
                        manifest_id,
                        size,
                        time_updated,
                    };
                    if let Err(e) =
                        super::workshop_manifest::upsert_installed_item(&manifest_path, app_id, record)
                    {
                        let _ = tx
                            .send(DownloadProgress {
                                state: DownloadProgressState::Failed,
                                current_file: format!(
                                    "content downloaded but failed updating workshop manifest: {e:#}"
                                ),
                                ..Default::default()
                            })
                            .await;
                        return;
                    }
                    let _ = tx
                        .send(DownloadProgress {
                            state: DownloadProgressState::Completed,
                            ..Default::default()
                        })
                        .await;
                }
                Err(e) => {
                    let _ = tx
                        .send(DownloadProgress {
                            state: DownloadProgressState::Failed,
                            current_file: format!("failed downloading Workshop item {published_file_id}: {e:#}"),
                            ..Default::default()
                        })
                        .await;
                }
            }
        });

        Ok(rx)
    }

    /// Read the installed Workshop items recorded in `appworkshop_<appid>.acf`.
    /// Purely local — no network, no session required.
    pub async fn read_installed_workshop(
        &self,
        app_id: u32,
    ) -> Result<Vec<crate::models::WorkshopInstalledInfo>> {
        let cfg = load_launcher_config().await?;
        let library_root = PathBuf::from(&cfg.steam_library_path);
        let path = super::workshop_manifest::workshop_manifest_path(&library_root, app_id);
        let items = super::workshop_manifest::read_workshop_manifest(&path)?;
        Ok(items
            .into_iter()
            .map(|i| crate::models::WorkshopInstalledInfo {
                id: i.published_file_id,
                manifest_id: i.manifest_id,
                size: i.size,
                time_updated: i.time_updated,
            })
            .collect())
    }

    /// Remove an installed Workshop item's content directory and its entry in
    /// `appworkshop_<appid>.acf`. Succeeds quietly if nothing is installed.
    pub async fn uninstall_workshop_item(&self, published_file_id: u64, app_id: u32) -> Result<()> {
        let cfg = load_launcher_config().await?;
        let library_root = PathBuf::from(&cfg.steam_library_path);
        let dir =
            super::workshop_manifest::workshop_content_dir(&library_root, app_id, published_file_id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("failed removing {}", dir.display()))?;
        }
        let path = super::workshop_manifest::workshop_manifest_path(&library_root, app_id);
        super::workshop_manifest::remove_installed_item(&path, app_id, published_file_id)?;
        tracing::info!("uninstalled Workshop item {published_file_id} (app {app_id})");
        Ok(())
    }
}
