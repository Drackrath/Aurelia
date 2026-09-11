//! Store-wide sales and events (marketing messages).
use super::*;
use steam_vent_proto::steammessages_marketingmessages_steamclient::{
    CMarketingMessages_GetActiveMarketingMessages_Request,
    CMarketingMessages_GetActiveMarketingMessages_Response,
};

/// One active store event or sale.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct StoreEvent {
    pub gid: u64,
    pub title: String,
    pub kind: String,
    pub start: u64,
    pub end: u64,
    pub associated_id: u32,
    pub associated_name: String,
    pub countries_allowed: String,
    pub countries_denied: String,
}

/// `k_EMarketingMessageWeekendDeal` → `WeekendDeal`.
fn kind_label(debug: &str) -> String {
    debug
        .strip_prefix("k_EMarketingMessage")
        .unwrap_or(debug)
        .to_string()
}

impl SteamClient {
    /// Active sales and events for `country`.
    pub async fn active_store_events(&self, country: &str) -> Result<Vec<StoreEvent>> {
        let connection = self.require_connection()?;
        let mut request = CMarketingMessages_GetActiveMarketingMessages_Request::new();
        request.set_country(country.to_string());
        let response: CMarketingMessages_GetActiveMarketingMessages_Response = connection
            .service_method(request)
            .await
            .context("failed calling MarketingMessages.GetActiveMarketingMessages")?;
        let mut events: Vec<StoreEvent> = response
            .messages
            .iter()
            .map(|m| StoreEvent {
                gid: m.gid(),
                title: m.title().to_string(),
                kind: kind_label(&format!("{:?}", m.type_())),
                start: u64::from(m.start_date()),
                end: u64::from(m.end_date()),
                associated_id: m.associated_id(),
                associated_name: m.associated_name().to_string(),
                countries_allowed: m.country_allow().to_string(),
                countries_denied: m.country_deny().to_string(),
            })
            .collect();
        events.sort_by_key(|e| e.end);
        Ok(events)
    }
}
