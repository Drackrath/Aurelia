//! StoreQuery RPCs: search, filtered query, similar.
//!
//! Generated from `proto/service_storequery.proto` (see `build.rs`); the
//! `RpcMessage`/`RpcMethod` impls are hand-written like `cloudconfig.rs`.
use super::*;
use std::io::{Read, Write};
use steam_vent_proto_common::{RpcMessage, RpcMethod};

include!(concat!(env!("OUT_DIR"), "/storequery/mod.rs"));

use service_storequery::{
    CStoreQueryFilters, CStoreQueryFilters_PriceFilters, CStoreQueryFilters_TypeFilters,
    CStoreQueryParams, CStoreQuery_MoreLikeThis_Request, CStoreQuery_MoreLikeThis_Response,
    CStoreQuery_Query_Request, CStoreQuery_Query_Response,
    CStoreQuery_SearchSuggestions_Request, CStoreQuery_SearchSuggestions_Response,
    StoreBrowseContext as QueryContext, StoreBrowseItemDataRequest as QueryDataRequest,
    StoreItemID as QueryItemID,
};

macro_rules! rpc_message {
    ($t:ty) => {
        impl RpcMessage for $t {
            fn parse(reader: &mut dyn Read) -> ::protobuf::Result<Self> {
                <Self as ::protobuf::Message>::parse_from_reader(reader)
            }
            fn write(&self, writer: &mut dyn Write) -> ::protobuf::Result<()> {
                use ::protobuf::Message;
                self.write_to_writer(writer)
            }
            fn encode_size(&self) -> usize {
                use ::protobuf::Message;
                self.compute_size() as usize
            }
        }
    };
}

rpc_message!(CStoreQuery_SearchSuggestions_Request);
rpc_message!(CStoreQuery_SearchSuggestions_Response);
rpc_message!(CStoreQuery_Query_Request);
rpc_message!(CStoreQuery_Query_Response);
rpc_message!(CStoreQuery_MoreLikeThis_Request);
rpc_message!(CStoreQuery_MoreLikeThis_Response);

impl RpcMethod for CStoreQuery_SearchSuggestions_Request {
    const METHOD_NAME: &'static str = "StoreQuery.SearchSuggestions#1";
    type Response = CStoreQuery_SearchSuggestions_Response;
}

impl RpcMethod for CStoreQuery_Query_Request {
    const METHOD_NAME: &'static str = "StoreQuery.Query#1";
    type Response = CStoreQuery_Query_Response;
}

impl RpcMethod for CStoreQuery_MoreLikeThis_Request {
    const METHOD_NAME: &'static str = "StoreQuery.MoreLikeThis#1";
    type Response = CStoreQuery_MoreLikeThis_Response;
}

/// Query result page: app ids plus totals.
#[derive(Debug, Clone, Default)]
pub struct QueryPage {
    pub app_ids: Vec<u32>,
    pub total: i32,
    pub suggestions: Vec<String>,
}

/// Store slice a `deals` query covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DealsScope {
    /// Discounted items among the regional top sellers.
    DiscountedTopSellers,
    /// Regional top sellers regardless of discount.
    TopSellers,
    /// Anything discounted at least `min_discount`.
    Specials,
}

fn query_context(language: &str, country: &str) -> QueryContext {
    let mut context = QueryContext::new();
    context.set_language(language.to_string());
    context.set_country_code(country.to_string());
    context
}

fn games_only() -> CStoreQueryFilters_TypeFilters {
    let mut types = CStoreQueryFilters_TypeFilters::new();
    types.set_include_apps(true);
    types.set_include_games(true);
    types
}

fn page_from(ids: &[QueryItemID], metadata: Option<&service_storequery::CStoreQueryResultMetadata>) -> QueryPage {
    QueryPage {
        app_ids: ids.iter().map(|i| i.appid()).filter(|&id| id != 0).collect(),
        total: metadata.map(|m| m.total_matching_records()).unwrap_or(0),
        suggestions: metadata.map(|m| m.spellcheck_suggestions.clone()).unwrap_or_default(),
    }
}

impl SteamClient {
    /// Search the store by title; app ids only.
    pub async fn search_store(
        &self,
        term: &str,
        max_results: u32,
        language: &str,
        country: &str,
    ) -> Result<QueryPage> {
        let connection = self.require_connection()?;
        let mut request = CStoreQuery_SearchSuggestions_Request::new();
        request.set_query_name("aurelia-search".to_string());
        request.context = MessageField::some(query_context(language, country));
        request.set_search_term(term.to_string());
        request.set_max_results(max_results);
        request.set_use_spellcheck(true);
        let mut data = QueryDataRequest::new();
        data.set_include_basic_info(true);
        request.data_request = MessageField::some(data);
        let response = connection
            .service_method(request)
            .await
            .context("failed calling StoreQuery.SearchSuggestions")?;
        Ok(page_from(&response.ids, response.metadata.as_ref()))
    }

    /// Regional top sellers or specials; ids only.
    pub async fn query_deals(
        &self,
        scope: DealsScope,
        min_discount: i32,
        start: i32,
        count: i32,
        language: &str,
        country: &str,
    ) -> Result<QueryPage> {
        let connection = self.require_connection()?;
        let mut filters = CStoreQueryFilters::new();
        filters.set_released_only(true);
        filters.type_filters = MessageField::some(games_only());
        match scope {
            DealsScope::TopSellers => filters.set_regional_top_n_sellers(500),
            DealsScope::DiscountedTopSellers => {
                filters.set_regional_top_n_sellers(500);
                let mut price = CStoreQueryFilters_PriceFilters::new();
                price.set_min_discount_percent(min_discount.max(1));
                filters.price_filters = MessageField::some(price);
            }
            DealsScope::Specials => {
                let mut price = CStoreQueryFilters_PriceFilters::new();
                price.set_min_discount_percent(min_discount.max(1));
                filters.price_filters = MessageField::some(price);
            }
        }
        let mut params = CStoreQueryParams::new();
        params.set_start(start);
        params.set_count(count);
        params.filters = MessageField::some(filters);

        let mut request = CStoreQuery_Query_Request::new();
        request.set_query_name("aurelia-deals".to_string());
        request.query = MessageField::some(params);
        request.context = MessageField::some(query_context(language, country));
        let response = connection
            .service_method(request)
            .await
            .context("failed calling StoreQuery.Query")?;
        Ok(page_from(&response.ids, response.metadata.as_ref()))
    }

    /// Items similar to `app_id`; app ids only.
    pub async fn similar_apps(
        &self,
        app_id: u32,
        count: i32,
        language: &str,
        country: &str,
    ) -> Result<QueryPage> {
        let connection = self.require_connection()?;
        let mut request = CStoreQuery_MoreLikeThis_Request::new();
        request.set_query_name("aurelia-similar".to_string());
        request.context = MessageField::some(query_context(language, country));
        let mut item = QueryItemID::new();
        item.set_appid(app_id);
        request.item_id = MessageField::some(item);
        request.set_count(count);
        let response = connection
            .service_method(request)
            .await
            .context("failed calling StoreQuery.MoreLikeThis")?;
        Ok(page_from(&response.ids, response.metadata.as_ref()))
    }
}
