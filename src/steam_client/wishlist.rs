//! Wishlist RPCs: read, count, add, remove.
//!
//! Generated from `proto/service_wishlist.proto` (see `build.rs`); the
//! `RpcMessage`/`RpcMethod` impls are hand-written like `cloudconfig.rs`.
use super::*;
use std::io::{Read, Write};
use steam_vent_proto_common::{RpcMessage, RpcMethod};

include!(concat!(env!("OUT_DIR"), "/wishlist/mod.rs"));

use service_wishlist::{
    CWishlist_AddToWishlist_Request, CWishlist_AddToWishlist_Response,
    CWishlist_GetWishlistItemCount_Request, CWishlist_GetWishlistItemCount_Response,
    CWishlist_GetWishlist_Request, CWishlist_GetWishlist_Response,
    CWishlist_RemoveFromWishlist_Request, CWishlist_RemoveFromWishlist_Response,
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

rpc_message!(CWishlist_GetWishlist_Request);
rpc_message!(CWishlist_GetWishlist_Response);
rpc_message!(CWishlist_GetWishlistItemCount_Request);
rpc_message!(CWishlist_GetWishlistItemCount_Response);
rpc_message!(CWishlist_AddToWishlist_Request);
rpc_message!(CWishlist_AddToWishlist_Response);
rpc_message!(CWishlist_RemoveFromWishlist_Request);
rpc_message!(CWishlist_RemoveFromWishlist_Response);

impl RpcMethod for CWishlist_GetWishlist_Request {
    const METHOD_NAME: &'static str = "Wishlist.GetWishlist#1";
    type Response = CWishlist_GetWishlist_Response;
}

impl RpcMethod for CWishlist_GetWishlistItemCount_Request {
    const METHOD_NAME: &'static str = "Wishlist.GetWishlistItemCount#1";
    type Response = CWishlist_GetWishlistItemCount_Response;
}

impl RpcMethod for CWishlist_AddToWishlist_Request {
    const METHOD_NAME: &'static str = "Wishlist.AddToWishlist#1";
    type Response = CWishlist_AddToWishlist_Response;
}

impl RpcMethod for CWishlist_RemoveFromWishlist_Request {
    const METHOD_NAME: &'static str = "Wishlist.RemoveFromWishlist#1";
    type Response = CWishlist_RemoveFromWishlist_Response;
}

/// One wishlist entry, lowest `priority` first.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct WishlistEntry {
    pub app_id: u32,
    pub priority: u32,
    pub date_added: u64,
}

impl SteamClient {
    /// Wishlist of `steam_id`, sorted by priority.
    pub async fn wishlist(&self, steam_id: u64) -> Result<Vec<WishlistEntry>> {
        let connection = self.require_connection()?;
        let mut request = CWishlist_GetWishlist_Request::new();
        request.set_steamid(steam_id);
        let response: CWishlist_GetWishlist_Response = connection
            .service_method(request)
            .await
            .context("failed calling Wishlist.GetWishlist")?;
        let mut items: Vec<WishlistEntry> = response
            .items
            .iter()
            .filter(|i| i.appid() != 0)
            .map(|i| WishlistEntry {
                app_id: i.appid(),
                priority: i.priority(),
                date_added: u64::from(i.date_added()),
            })
            .collect();
        // Priority 0 means "unranked"; those go last.
        items.sort_by_key(|e| (e.priority == 0, e.priority, std::cmp::Reverse(e.date_added)));
        Ok(items)
    }

    /// Number of items on `steam_id`'s wishlist.
    pub async fn wishlist_count(&self, steam_id: u64) -> Result<u32> {
        let connection = self.require_connection()?;
        let mut request = CWishlist_GetWishlistItemCount_Request::new();
        request.set_steamid(steam_id);
        let response: CWishlist_GetWishlistItemCount_Response = connection
            .service_method(request)
            .await
            .context("failed calling Wishlist.GetWishlistItemCount")?;
        Ok(response.count())
    }

    /// Add to own wishlist; returns new count.
    pub async fn wishlist_add(&self, app_id: u32) -> Result<u32> {
        let connection = self.require_connection()?;
        let mut request = CWishlist_AddToWishlist_Request::new();
        request.set_appid(app_id);
        let response: CWishlist_AddToWishlist_Response = connection
            .service_method(request)
            .await
            .context("failed calling Wishlist.AddToWishlist")?;
        Ok(response.wishlist_count())
    }

    /// Remove from own wishlist; returns new count.
    pub async fn wishlist_remove(&self, app_id: u32) -> Result<u32> {
        let connection = self.require_connection()?;
        let mut request = CWishlist_RemoveFromWishlist_Request::new();
        request.set_appid(app_id);
        let response: CWishlist_RemoveFromWishlist_Response = connection
            .service_method(request)
            .await
            .context("failed calling Wishlist.RemoveFromWishlist")?;
        Ok(response.wishlist_count())
    }
}
