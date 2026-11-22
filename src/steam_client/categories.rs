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
        if !names.is_empty()
            && let Err(e) = save_tag_name_cache(&cache_key(language), &names).await
        {
            tracing::debug!("could not cache category names: {e:#}");
        }
        Ok(names)
    }
}

/// Steam store category IDs that denote *online* multiplayer (as opposed to
/// local/LAN play). A title carrying any of these effectively needs a network
/// connection for its core multiplayer experience.
/// 20 = MMO, 36 = Online PvP, 38 = Online Co-op.
pub(super) const ONLINE_MULTIPLAYER_CATEGORIES: &[u32] = &[20, 36, 38];

/// Steam store category ID for Single-player.
pub(super) const SINGLE_PLAYER_CATEGORY: u32 = 2;

/// Infer whether a game requires an online connection from its PICS `common`
/// store-category map (keyed `category_<id>`).
///
/// Steam has no dedicated "online required" field, so this is a heuristic: a
/// title is treated as online-required when it advertises an online-multiplayer
/// category (MMO / Online PvP / Online Co-op) but does *not* advertise
/// single-player support — i.e. there is no documented way to play it offline.
pub fn category_online_required(categories: &HashMap<String, String>) -> bool {
    let has = |id: u32| {
        categories
            .get(&format!("category_{id}"))
            .map(|v| v != "0")
            .unwrap_or(false)
    };
    let online = ONLINE_MULTIPLAYER_CATEGORIES.iter().any(|&id| has(id));
    online && !has(SINGLE_PLAYER_CATEGORY)
}
