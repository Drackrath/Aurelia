//! Store category names via `StoreBrowse.GetStoreCategories`.
use super::*;
use crate::core::config::{load_tag_name_cache, save_tag_name_cache};
use steam_vent_proto::steammessages_storebrowse_steamclient::{
    CStoreBrowse_GetStoreCategories_Request, CStoreBrowse_GetStoreCategories_Response,
};

/// Cache namespace, shares the tag-name cache format.
fn cache_key(language: &str) -> String {
    format!("categories-{language}")
}

impl SteamClient {
    /// Category id → localized name, disk-cached per language.
    pub async fn store_category_names(&self, language: &str) -> Result<HashMap<u32, String>> {
        let cached = load_tag_name_cache(&cache_key(language)).await;
        if !cached.is_empty() {
            return Ok(cached);
        }
        let connection = self.require_connection()?;
        let mut request = CStoreBrowse_GetStoreCategories_Request::new();
        request.set_language(language.to_string());
        let response: CStoreBrowse_GetStoreCategories_Response = connection
            .service_method(request)
            .await
            .context("failed calling StoreBrowse.GetStoreCategories")?;
        let names: HashMap<u32, String> = response
            .categories
            .iter()
            .filter(|c| c.categoryid() != 0 && !c.display_name().is_empty())
            .map(|c| (c.categoryid(), c.display_name().to_string()))
            .collect();
        if !names.is_empty() {
            if let Err(e) = save_tag_name_cache(&cache_key(language), &names).await {
                tracing::debug!("could not cache category names: {e:#}");
            }
        }
        Ok(names)
    }
}
