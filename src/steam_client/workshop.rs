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

use crate::models::{WorkshopComment, WorkshopItem, WorkshopItemKind};
use steam_vent_proto::enums::ECommentThreadType;
use steam_vent_proto::steammessages_clientserver_ucm::CMsgClientUCMEnumerateUserSubscribedFilesWithUpdates;
use steam_vent_proto::steammessages_community_steamclient::{
    CCommunity_GetCommentThread_Request, CCommunity_GetCommentThread_Response,
    CCommunity_PostCommentToThread_Request, CCommunity_PostCommentToThread_Response,
};
use steam_vent_proto::steammessages_publishedfile_steamclient::{
    CPublishedFile_GetDetails_Request, CPublishedFile_GetDetails_Response,
    CPublishedFile_QueryFiles_Request, CPublishedFile_QueryFiles_Response,
    CPublishedFile_Subscribe_Request, CPublishedFile_Subscribe_Response,
    CPublishedFile_Unsubscribe_Request, CPublishedFile_Unsubscribe_Response,
    CPublishedFile_Vote_Request, CPublishedFile_Vote_Response, PublishedFileDetails,
};

/// Maximum depth `expand_collections` will recurse into nested collections
/// before giving up, as a backstop in addition to the visited-set cycle guard.
const MAX_COLLECTION_DEPTH: usize = 8;

/// Map a `PublishedFileDetails` (from GetDetails or QueryFiles) into our
/// [`WorkshopItem`]. `fallback_app_id` is used when the detail carries no
/// `consumer_appid` (QueryFiles results sometimes omit it — we know the app we
/// queried). The caller is responsible for skipping non-OK / zero-id details.
fn detail_to_workshop_item(detail: &PublishedFileDetails, fallback_app_id: u32) -> WorkshopItem {
    let children: Vec<u64> = detail
        .children
        .iter()
        .map(|c| c.publishedfileid())
        .filter(|&cid| cid != 0)
        .collect();
    // A collection is a Workshop entry whose payload is a list of member ids.
    let kind = if !children.is_empty() || detail.num_children() > 0 {
        WorkshopItemKind::Collection
    } else {
        WorkshopItemKind::Item
    };
    let app_id = match detail.consumer_appid() {
        0 => fallback_app_id,
        a => a,
    };
    WorkshopItem {
        id: detail.publishedfileid(),
        app_id,
        creator: detail.creator(),
        title: detail.title().to_string(),
        hcontent_file: detail.hcontent_file(),
        file_url: detail.file_url().to_string(),
        file_size: detail.file_size(),
        // proto field is `uint32`; widen to the model's `i64`.
        time_updated: i64::from(detail.time_updated()),
        kind,
        children,
    }
}

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
            out.push(detail_to_workshop_item(detail, 0));
        }

        Ok(out)
    }

    /// Browse/search a game's Workshop via `PublishedFile.QueryFiles`. Returns one
    /// page of results plus the total match count and a `next_cursor` for paging.
    ///
    /// - `query_type` is an `EPublishedFileQueryType` (e.g. 3 = RankedByTrend,
    ///   0 = RankedByVote, 1 = RankedByPublicationDate, 9 = TotalUniqueSubscriptions,
    ///   12 = RankedByTextSearch, 21 = RankedByLastUpdatedDate).
    /// - `cursor` is `"*"` for the first page, or a previous page's `next_cursor`.
    /// - `numperpage` is clamped to Steam's 1..=100 range.
    pub async fn query_workshop_files(
        &self,
        app_id: u32,
        search_text: Option<&str>,
        query_type: u32,
        cursor: &str,
        numperpage: u32,
        required_tags: &[String],
    ) -> Result<crate::models::WorkshopQueryPage> {
        let connection = self.connection.as_ref().ok_or_else(|| anyhow!("No connection"))?;

        let mut request = CPublishedFile_QueryFiles_Request::new();
        request.set_appid(app_id);
        request.set_query_type(query_type);
        request.set_cursor(cursor.to_string());
        request.set_numperpage(numperpage.clamp(1, 100));
        if let Some(text) = search_text.filter(|t| !t.is_empty()) {
            request.set_search_text(text.to_string());
        }
        if !required_tags.is_empty() {
            request.requiredtags = required_tags.to_vec();
        }
        // Ask for full per-item details (title/size/manifest/consumer app) and
        // collection children so results map to complete `WorkshopItem`s.
        request.set_return_details(true);
        request.set_return_children(true);

        let response: CPublishedFile_QueryFiles_Response = connection
            .service_method(request)
            .await
            .context("failed calling PublishedFile.QueryFiles")?;

        let items: Vec<WorkshopItem> = response
            .publishedfiledetails
            .iter()
            .filter(|d| d.publishedfileid() != 0)
            .map(|d| detail_to_workshop_item(d, app_id))
            .collect();

        Ok(crate::models::WorkshopQueryPage {
            items,
            total: response.total(),
            next_cursor: response.next_cursor().to_string(),
        })
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

    /// Rate a Workshop item — `up` for thumbs-up, `false` for thumbs-down
    /// (`PublishedFile.Vote`).
    pub async fn vote_workshop_item(&self, id: u64, up: bool) -> Result<()> {
        let connection = self.connection.as_ref().ok_or_else(|| anyhow!("No connection"))?;

        let mut request = CPublishedFile_Vote_Request::new();
        request.set_publishedfileid(id);
        request.set_vote_up(up);

        let _response: CPublishedFile_Vote_Response = connection
            .service_method(request)
            .await
            .with_context(|| format!("failed voting on Workshop item {id}"))?;

        tracing::info!("voted {} on Workshop item {id}", if up { "up" } else { "down" });
        Ok(())
    }

    /// Resolve the SteamID64 of a published file's creator — needed to address its
    /// comment thread (the thread is keyed by owner steamid + publishedfileid).
    async fn workshop_item_creator(&self, id: u64) -> Result<u64> {
        let items = self.fetch_published_file_details(&[id]).await?;
        let creator = items
            .first()
            .map(|i| i.creator)
            .filter(|&c| c != 0)
            .with_context(|| format!("could not resolve the creator of Workshop item {id}"))?;
        Ok(creator)
    }

    /// Read a page of comments on a Workshop item's public comment thread
    /// (`Community.GetCommentThread`). `start`/`count` page through the thread
    /// (count is clamped to a sane range).
    pub async fn workshop_comments(
        &self,
        id: u64,
        start: i32,
        count: i32,
    ) -> Result<Vec<WorkshopComment>> {
        let connection = self.connection.as_ref().ok_or_else(|| anyhow!("No connection"))?;
        let owner = self.workshop_item_creator(id).await?;

        let mut request = CCommunity_GetCommentThread_Request::new();
        request.set_steamid(owner);
        request.set_comment_thread_type(ECommentThreadType::k_ECommentThreadTypePublishedFile_Public);
        request.set_gidfeature(id);
        request.set_start(start.max(0));
        request.set_count(count.clamp(1, 100));

        let response: CCommunity_GetCommentThread_Response = connection
            .service_method(request)
            .await
            .with_context(|| format!("failed fetching comments for Workshop item {id}"))?;

        let comments = response
            .comments
            .iter()
            .map(|c| WorkshopComment {
                id: c.gidcomment(),
                author: c.steamid(),
                timestamp: i64::from(c.timestamp()),
                text: c.text().to_string(),
                upvotes: c.upvotes(),
            })
            .collect();
        Ok(comments)
    }

    /// Post a comment to a Workshop item's public comment thread
    /// (`Community.PostCommentToThread`). Returns the new comment's `gidcomment`.
    pub async fn post_workshop_comment(&self, id: u64, text: &str) -> Result<u64> {
        let connection = self.connection.as_ref().ok_or_else(|| anyhow!("No connection"))?;
        let owner = self.workshop_item_creator(id).await?;

        let mut request = CCommunity_PostCommentToThread_Request::new();
        request.set_steamid(owner);
        request.set_comment_thread_type(ECommentThreadType::k_ECommentThreadTypePublishedFile_Public);
        request.set_gidfeature(id);
        request.set_text(text.to_string());

        let response: CCommunity_PostCommentToThread_Response = connection
            .service_method(request)
            .await
            .with_context(|| format!("failed posting a comment to Workshop item {id}"))?;

        let gid = response.gidcomment();
        tracing::info!("posted comment {gid} to Workshop item {id}");
        Ok(gid)
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
                    let snapshot = progress_state.read().ok().map(|s| {
                        (
                            s.is_downloading,
                            s.downloaded_bytes,
                            s.total_bytes,
                            s.status_text.clone(),
                            s.depot_id,
                            s.depot_downloaded_bytes,
                            s.depot_total_bytes,
                        )
                    });
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
