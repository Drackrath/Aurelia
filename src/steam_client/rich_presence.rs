//! Rich presence upload and lookup.
use super::*;
use crate::steam_client::binary_kv;
use std::io::Write;
use steam_vent::NetMessageHeader;
use steam_vent_core::EncodableMessage;
use steam_vent_proto::steammessages_clientserver_2::{
    CMsgClientRichPresenceInfo, CMsgClientRichPresenceRequest, CMsgClientRichPresenceUpload,
};
use steam_vent_proto_common::RpcMessageWithKind;
use tokio_stream::StreamExt;

/// Top section name Steam uses.
pub const RP_SECTION: &str = "RP";
/// Wait for a friend's answer.
const RP_WAIT: Duration = Duration::from_secs(10);

/// Binary KeyValues `RP` section.
pub fn encode_kv(pairs: &[(String, String)]) -> Vec<u8> {
    binary_kv::encode(RP_SECTION, pairs)
}

/// Key/value pairs; None when malformed.
pub fn decode_kv(bytes: &[u8]) -> Option<Vec<(String, String)>> {
    binary_kv::decode(bytes)
}

/// Message sent with a routing app.
#[derive(Debug)]
pub(crate) struct AppRouted<M> {
    pub(crate) msg: M,
    pub(crate) app_id: u32,
}

impl<M: RpcMessageWithKind + Send> EncodableMessage for AppRouted<M> {
    fn write_body<W: Write>(&self, writer: W) -> std::io::Result<()> {
        self.msg.write_body(writer)
    }

    fn encode_size(&self) -> usize {
        self.msg.encode_size()
    }

    fn process_header(&self, header: &mut NetMessageHeader) {
        header.source_app_id = Some(self.app_id);
    }
}

impl<M: RpcMessageWithKind + Send> steam_vent::SendableMessage for AppRouted<M> {
    type KindEnum = M::KindEnum;

    fn kind(&self) -> Self::KindEnum {
        M::KIND
    }

    fn is_protobuf(&self) -> bool {
        true
    }
}

impl SteamClient {
    /// Replace the app's rich presence.
    pub async fn upload_rich_presence(
        &self,
        app_id: u32,
        pairs: &[(String, String)],
    ) -> Result<()> {
        let connection = self.require_connection()?;
        let mut msg = CMsgClientRichPresenceUpload::new();
        msg.set_rich_presence_kv(encode_kv(pairs));
        connection
            .send(AppRouted { msg, app_id })
            .await
            .map_err(|e| anyhow!("rich presence upload failed: {e}"))
    }

    /// A user's rich presence, from Steam.
    pub async fn friend_rich_presence(&self, steam_id: u64) -> Result<Vec<(String, String)>> {
        let connection = self.require_connection()?;
        let mut answers = connection.on::<CMsgClientRichPresenceInfo>();
        let mut req = CMsgClientRichPresenceRequest::new();
        req.steamid_request.push(steam_id);
        connection
            .send(req)
            .await
            .map_err(|e| anyhow!("rich presence request failed: {e}"))?;
        let wait = async {
            while let Some(Ok(info)) = answers.next().await {
                let found = info
                    .rich_presence
                    .iter()
                    .find(|rp| rp.steamid_user() == steam_id);
                if let Some(rp) = found {
                    return decode_kv(rp.rich_presence_kv())
                        .ok_or_else(|| anyhow!("malformed rich presence from Steam"));
                }
            }
            bail!("connection closed before rich presence arrived")
        };
        tokio::time::timeout(RP_WAIT, wait)
            .await
            .map_err(|_| anyhow!("Steam sent no rich presence for {steam_id}"))?
    }
}

#[cfg(test)]
#[path = "rich_presence_tests.rs"]
mod tests;
