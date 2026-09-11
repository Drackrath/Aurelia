//! Store tag names: baked table, cache, then CM.
use super::*;
use crate::core::config::{load_tag_name_cache, save_tag_name_cache};
use steam_vent_proto::steammessages_store_steamclient::{
    CStore_GetLocalizedNameForTags_Request, CStore_GetLocalizedNameForTags_Response,
    CStore_GetTagList_Request, CStore_GetTagList_Response,
};

impl SteamClient {
    /// Full tag vocabulary in `language`, sorted by id.
    pub async fn fetch_tag_list(&self, language: &str) -> Result<Vec<(u32, String)>> {
        let connection = self.require_connection()?;
        let mut request = CStore_GetTagList_Request::new();
        request.set_language(language.to_string());
        let response: CStore_GetTagList_Response = connection
            .service_method(request)
            .await
            .context("failed calling Store.GetTagList")?;
        let mut tags: Vec<(u32, String)> = response
            .tags
            .iter()
            .filter(|t| t.tagid() != 0 && !t.name().is_empty())
            .map(|t| (t.tagid(), t.name().to_string()))
            .collect();
        tags.sort_by_key(|(id, _)| *id);
        tags.dedup_by_key(|(id, _)| *id);
        Ok(tags)
    }

    /// Localized names for `ids` over the CM.
    pub async fn fetch_tag_names(
        &self,
        ids: &[u32],
        language: &str,
    ) -> Result<HashMap<u32, String>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let connection = self.require_connection()?;
        let mut request = CStore_GetLocalizedNameForTags_Request::new();
        request.set_language(language.to_string());
        request.tagids = ids.to_vec();
        let response: CStore_GetLocalizedNameForTags_Response = connection
            .service_method(request)
            .await
            .context("failed calling Store.GetLocalizedNameForTags")?;
        Ok(response
            .tags
            .iter()
            .filter(|t| t.tagid() != 0)
            .map(|t| {
                let name = if t.name().is_empty() { t.english_name() } else { t.name() };
                (t.tagid(), name.to_string())
            })
            .filter(|(_, name)| !name.is_empty())
            .collect())
    }

    /// Names for `ids`: table, disk cache, then CM.
    pub async fn resolve_tag_names(&self, ids: &[u32], language: &str) -> HashMap<u32, String> {
        let english = language.eq_ignore_ascii_case("english");
        let mut names: HashMap<u32, String> = HashMap::new();
        if english {
            for &id in ids {
                if let Some(name) = tags_table::tag_name(id) {
                    names.insert(id, name.to_string());
                }
            }
        }
        let mut cached = if names.len() == ids.len() {
            HashMap::new()
        } else {
            load_tag_name_cache(language).await
        };
        for &id in ids {
            if let Some(name) = cached.get(&id) {
                names.entry(id).or_insert_with(|| name.clone());
            }
        }
        let missing: Vec<u32> = ids.iter().copied().filter(|id| !names.contains_key(id)).collect();
        if missing.is_empty() {
            return names;
        }
        match self.fetch_tag_names(&missing, language).await {
            Ok(fetched) if !fetched.is_empty() => {
                cached.extend(fetched.iter().map(|(k, v)| (*k, v.clone())));
                if let Err(e) = save_tag_name_cache(language, &cached).await {
                    tracing::debug!("could not cache tag names: {e:#}");
                }
                names.extend(fetched);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("tag names unavailable from Steam: {e:#}"),
        }
        // Last resort: English table for any language.
        for &id in &missing {
            if let Some(name) = tags_table::tag_name(id) {
                names.entry(id).or_insert_with(|| name.to_string());
            }
        }
        names
    }
}

#[cfg(test)]
#[path = "tags_tests.rs"]
mod tests;
