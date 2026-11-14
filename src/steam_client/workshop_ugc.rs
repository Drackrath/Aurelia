//! Workshop data for steamclient.so games.
//!
//! Full item details, queries and
//! local install state.

use super::*;

use crate::core::models::{DownloadProgressState, DownloadState};
use steam_vent_proto::steammessages_publishedfile_steamclient::{
    CPublishedFile_GetDetails_Request, CPublishedFile_GetDetails_Response,
    CPublishedFile_GetUserFiles_Request, CPublishedFile_GetUserFiles_Response,
    CPublishedFile_Publish_Request, CPublishedFile_Publish_Response,
    CPublishedFile_QueryFiles_Request, CPublishedFile_QueryFiles_Response,
    CPublishedFile_Update_Request, CPublishedFile_Update_Response, PublishedFileDetails,
};

/// SteamID64 of individual account 0.
const INDIVIDUAL_BASE: u64 = 76_561_197_960_265_728;

/// One extra preview of an item.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UgcPreview {
    /// Image URL or YouTube video ID.
    pub url: String,
    pub file_name: String,
    /// `EItemPreviewType`.
    pub kind: u32,
}

/// Fields a game changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UgcUpdate {
    pub id: u64,
    pub title: Option<String>,
    pub description: Option<String>,
    pub visibility: Option<u32>,
    pub tags: Option<Vec<String>>,
}

/// `SteamUGCDetails_t` fields we fill.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UgcDetails {
    pub id: u64,
    /// `EResult`; 1 is OK.
    pub result: i32,
    pub file_type: u32,
    pub creator_app: u32,
    pub consumer_app: u32,
    pub title: String,
    pub description: String,
    pub owner: u64,
    pub created: u32,
    pub updated: u32,
    pub visibility: u32,
    pub banned: bool,
    pub accepted: bool,
    /// Comma-separated tag names.
    pub tags: String,
    /// Content manifest (`m_hFile`).
    pub file: u64,
    pub preview_file: u64,
    pub file_name: String,
    pub file_size: u64,
    pub preview_size: u64,
    /// Preview image URL.
    pub url: String,
    pub votes_up: u32,
    pub votes_down: u32,
    pub score: f32,
    pub children: u32,
    pub metadata: String,
    pub kv_tags: Vec<(String, String)>,
    pub previews: Vec<UgcPreview>,
}

/// One page of query results.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UgcPage {
    pub total: u32,
    pub next_cursor: String,
    pub items: Vec<UgcDetails>,
}

/// Local state of one item.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UgcInstall {
    pub installed: bool,
    /// Installed manifest differs.
    pub needs_update: bool,
    pub size: u64,
    pub timestamp: u32,
    pub folder: String,
}

/// Maps Steam's details to ours.
pub fn details_from_proto(d: &PublishedFileDetails) -> UgcDetails {
    let tags: Vec<&str> = d
        .tags
        .iter()
        .map(|t| t.tag())
        .filter(|t| !t.is_empty())
        .collect();
    let description = if d.file_description().is_empty() {
        d.short_description()
    } else {
        d.file_description()
    };
    UgcDetails {
        id: d.publishedfileid(),
        result: i32::try_from(d.result()).unwrap_or(2),
        file_type: d.file_type(),
        creator_app: d.creator_appid(),
        consumer_app: d.consumer_appid(),
        title: d.title().to_string(),
        description: description.to_string(),
        owner: d.creator(),
        created: d.time_created(),
        updated: d.time_updated(),
        visibility: d.visibility(),
        banned: d.banned(),
        accepted: d.workshop_accepted(),
        tags: tags.join(","),
        file: d.hcontent_file(),
        preview_file: d.hcontent_preview(),
        file_name: d.filename().to_string(),
        file_size: d.file_size(),
        preview_size: d.preview_file_size(),
        url: d.preview_url().to_string(),
        votes_up: d.vote_data.votes_up(),
        votes_down: d.vote_data.votes_down(),
        score: d.vote_data.score(),
        children: d.num_children(),
        metadata: d.metadata().to_string(),
        kv_tags: d
            .kvtags
            .iter()
            .map(|t| (t.key().to_string(), t.value().to_string()))
            .collect(),
        previews: d
            .previews
            .iter()
            .map(|p| UgcPreview {
                url: if p.youtubevideoid().is_empty() {
                    p.url().to_string()
                } else {
                    p.youtubevideoid().to_string()
                },
                file_name: p.filename().to_string(),
                kind: p.preview_type(),
            })
            .collect(),
    }
}

/// Page from a details list.
fn page_from(total: u32, cursor: &str, details: &[PublishedFileDetails]) -> UgcPage {
    UgcPage {
        total,
        next_cursor: cursor.to_string(),
        items: details
            .iter()
            .filter(|d| d.publishedfileid() != 0)
            .map(details_from_proto)
            .collect(),
    }
}

/// Installed record versus Steam's manifest.
pub(crate) fn install_state(
    record: Option<&super::workshop_manifest::InstalledWorkshopItem>,
    folder: &Path,
    current_manifest: Option<u64>,
) -> UgcInstall {
    let Some(record) = record.filter(|_| folder.is_dir()) else {
        return UgcInstall::default();
    };
    UgcInstall {
        installed: true,
        needs_update: current_manifest.is_some_and(|m| m != 0 && m != record.manifest_id),
        size: record.size,
        timestamp: u32::try_from(record.time_updated).unwrap_or(0),
        folder: folder.to_string_lossy().into_owned(),
    }
}

impl SteamClient {
    /// Details with votes, tags, descriptions.
    pub async fn ugc_details(&self, ids: &[u64]) -> Result<Vec<UgcDetails>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.require_connection()?;
        let mut request = CPublishedFile_GetDetails_Request::new();
        request.publishedfileids = ids.to_vec();
        request.set_includetags(true);
        request.set_includevotes(true);
        request.set_includechildren(true);
        request.set_includekvtags(true);
        request.set_includeadditionalpreviews(true);
        request.set_includemetadata(true);
        let response: CPublishedFile_GetDetails_Response = connection
            .service_method(request)
            .await
            .context("failed calling PublishedFile.GetDetails")?;
        Ok(response
            .publishedfiledetails
            .iter()
            .map(details_from_proto)
            .collect())
    }

    /// One page by page or cursor.
    #[allow(clippy::too_many_arguments)]
    pub async fn ugc_query(
        &self,
        app_id: u32,
        query_type: u32,
        page: u32,
        cursor: Option<&str>,
        search: Option<&str>,
        tags: &[String],
        excluded: &[String],
        per_page: u32,
    ) -> Result<UgcPage> {
        let connection = self.require_connection()?;
        let mut request = CPublishedFile_QueryFiles_Request::new();
        request.set_appid(app_id);
        request.set_query_type(query_type);
        request.set_numperpage(per_page.clamp(1, 50));
        match cursor {
            Some(cursor) => request.set_cursor(cursor.to_string()),
            None => request.set_page(page.max(1)),
        }
        if let Some(text) = search.filter(|t| !t.is_empty()) {
            request.set_search_text(text.to_string());
        }
        request.requiredtags = tags.to_vec();
        request.excludedtags = excluded.to_vec();
        request.set_return_metadata(true);
        request.set_return_kv_tags(true);
        request.set_return_previews(true);
        request.set_return_details(true);
        request.set_return_tags(true);
        request.set_return_vote_data(true);
        request.set_return_children(true);
        let response: CPublishedFile_QueryFiles_Response = connection
            .service_method(request)
            .await
            .context("failed calling PublishedFile.QueryFiles")?;
        Ok(page_from(
            response.total(),
            response.next_cursor(),
            &response.publishedfiledetails,
        ))
    }

    /// Items an account published.
    pub async fn ugc_published_by(
        &self,
        app_id: u32,
        account: u32,
        page: u32,
        tags: &[String],
        excluded: &[String],
        per_page: u32,
    ) -> Result<UgcPage> {
        let connection = self.require_connection()?;
        let mut request = CPublishedFile_GetUserFiles_Request::new();
        request.set_steamid(INDIVIDUAL_BASE + u64::from(account));
        request.set_appid(app_id);
        request.set_page(page.max(1));
        request.set_numperpage(per_page.clamp(1, 50));
        request.set_type("myfiles".to_string());
        request.requiredtags = tags.to_vec();
        request.excludedtags = excluded.to_vec();
        request.set_return_tags(true);
        request.set_return_vote_data(true);
        request.set_return_kv_tags(true);
        request.set_return_previews(true);
        request.set_return_children(true);
        request.set_return_metadata(true);
        let response: CPublishedFile_GetUserFiles_Response = connection
            .service_method(request)
            .await
            .context("failed calling PublishedFile.GetUserFiles")?;
        Ok(page_from(
            response.total(),
            "",
            &response.publishedfiledetails,
        ))
    }

    /// Creates an empty item.
    pub async fn ugc_create_item(&self, app_id: u32, file_type: u32) -> Result<u64> {
        let connection = self.require_connection()?;
        let mut request = CPublishedFile_Publish_Request::new();
        request.set_appid(app_id);
        request.set_consumer_appid(app_id);
        request.set_file_type(file_type);
        let response: CPublishedFile_Publish_Response = connection
            .service_method(request)
            .await
            .context("failed calling PublishedFile.Publish")?;
        match response.publishedfileid() {
            0 => bail!("Steam created no Workshop item"),
            id => Ok(id),
        }
    }

    /// Applies title, text, visibility, tags.
    pub async fn ugc_update_item(&self, app_id: u32, update: &UgcUpdate) -> Result<()> {
        let connection = self.require_connection()?;
        let mut request = CPublishedFile_Update_Request::new();
        request.set_appid(app_id);
        request.set_publishedfileid(update.id);
        if let Some(title) = &update.title {
            request.set_title(title.clone());
        }
        if let Some(text) = &update.description {
            request.set_file_description(text.clone());
        }
        if let Some(visibility) = update.visibility {
            request.set_visibility(visibility);
        }
        if let Some(tags) = &update.tags {
            request.tags = tags.clone();
        }
        let _: CPublishedFile_Update_Response = connection
            .service_method(request)
            .await
            .context("failed calling PublishedFile.Update")?;
        Ok(())
    }

    /// Install state; manifest from Steam.
    pub async fn ugc_install_state(&self, app_id: u32, id: u64) -> Result<UgcInstall> {
        let cfg = load_launcher_config().await?;
        let root = PathBuf::from(&cfg.steam_library_path);
        let path = super::workshop_manifest::workshop_manifest_path(&root, app_id);
        let items = super::workshop_manifest::read_workshop_manifest(&path)?;
        let record = items.iter().find(|i| i.published_file_id == id);
        let folder = super::workshop_manifest::workshop_content_dir(&root, app_id, id);
        let current = match record {
            Some(_) => self.ugc_details(&[id]).await.ok().and_then(|d| {
                d.into_iter()
                    .find(|d| d.id == id && d.result == 1)
                    .map(|d| d.file)
            }),
            None => None,
        };
        Ok(install_state(record, &folder, current))
    }

    /// Downloads one item; waits.
    pub async fn ugc_download(
        &self,
        app_id: u32,
        id: u64,
        progress: Arc<std::sync::RwLock<DownloadState>>,
    ) -> Result<()> {
        let item = self
            .fetch_published_file_details(&[id])
            .await?
            .into_iter()
            .find(|i| i.id == id)
            .with_context(|| format!("Workshop item {id} not found"))?;
        if item.app_id != app_id {
            bail!("Workshop item {id} belongs to app {}", item.app_id);
        }
        let mut rx = self.install_workshop_item(&item, progress).await?;
        while let Some(update) = rx.recv().await {
            match update.state {
                DownloadProgressState::Completed => return Ok(()),
                DownloadProgressState::Failed => {
                    bail!("Workshop item {id}: {}", update.current_file);
                }
                _ => {}
            }
        }
        bail!("Workshop item {id}: download ended early")
    }
}

#[cfg(test)]
#[path = "workshop_ugc_tests.rs"]
mod tests;
