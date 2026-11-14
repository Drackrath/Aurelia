//! In-game purchases approved in a browser.
use super::*;
use crate::steam_client::binary_kv;
use bytes::BytesMut;
use std::sync::Mutex;
use steam_vent::NetMessageHeader;
use steam_vent_core::{DecodableMessage, ReceivableMessage};
use steam_vent_proto::enums_clientserver::EMsg;
use tokio_stream::StreamExt;

/// Pending orders kept at most.
const MAX_PENDING: usize = 32;

/// A purchase Steam wants approved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicroTxnRequest {
    pub app_id: u32,
    pub order_id: u64,
    pub trans_id: u64,
}

impl MicroTxnRequest {
    /// Steam's web checkout for it.
    pub fn approval_url(&self) -> String {
        format!(
            "https://checkout.steampowered.com/checkout/approvetxn/{}/",
            self.trans_id
        )
    }
}

fn field<T: std::str::FromStr>(pairs: &[(String, String)], name: &str) -> Option<T> {
    pairs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| v.trim().parse().ok())
}

/// Parses `ClientMicroTxnAuthRequest`'s body.
pub fn parse_auth_request(body: &[u8]) -> Option<MicroTxnRequest> {
    // One leading byte, then order details.
    let (order, _) = binary_kv::decode_prefix(body.get(1..)?)?;
    let request = MicroTxnRequest {
        app_id: field(&order, "appid")?,
        order_id: field(&order, "orderid")?,
        trans_id: field(&order, "transid")?,
    };
    (request.order_id != 0 && request.trans_id != 0).then_some(request)
}

/// Raw `ClientMicroTxnAuthRequest` message.
#[derive(Debug)]
struct AuthRequestMsg(Option<MicroTxnRequest>);

impl DecodableMessage for AuthRequestMsg {
    fn read_body(data: BytesMut, _header: &NetMessageHeader) -> std::io::Result<Self> {
        Ok(Self(parse_auth_request(&data)))
    }
}

impl ReceivableMessage for AuthRequestMsg {
    type KindEnum = EMsg;
    const KIND: EMsg = EMsg::k_EMsgClientMicroTxnAuthRequest;
    const IS_PROTOBUF: bool = false;
}

static PENDING: Mutex<Vec<MicroTxnRequest>> = Mutex::new(Vec::new());

fn pending_lock() -> std::sync::MutexGuard<'static, Vec<MicroTxnRequest>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Remember an order awaiting approval.
pub fn record(request: MicroTxnRequest) {
    let mut pending = pending_lock();
    pending.retain(|p| p.order_id != request.order_id);
    if pending.len() == MAX_PENDING {
        pending.remove(0);
    }
    pending.push(request);
}

/// Orders awaiting approval.
pub fn pending() -> Vec<MicroTxnRequest> {
    pending_lock().clone()
}

/// Remove and return one order.
pub fn take(order_id: u64) -> Option<MicroTxnRequest> {
    let mut pending = pending_lock();
    let at = pending.iter().position(|p| p.order_id == order_id)?;
    Some(pending.remove(at))
}

impl SteamClient {
    /// Report purchase prompts Steam sends.
    pub async fn run_microtxn_watcher(
        &self,
        on_request: impl Fn(MicroTxnRequest) + Send,
    ) -> Result<()> {
        let connection = self.require_connection_owned()?;
        let mut requests = connection.on::<AuthRequestMsg>();
        while let Some(item) = requests.next().await {
            match item {
                Ok(AuthRequestMsg(Some(request))) => {
                    record(request.clone());
                    on_request(request);
                }
                Ok(AuthRequestMsg(None)) => {
                    tracing::warn!("Steam sent a purchase prompt Aurelia cannot read")
                }
                Err(_) => continue,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "microtxn_tests.rs"]
mod tests;
