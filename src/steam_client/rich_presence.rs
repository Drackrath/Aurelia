//! Rich presence upload and lookup.
use super::*;
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

/// Binary KeyValues type bytes.
mod kv {
    pub const SECTION: u8 = 0;
    pub const STRING: u8 = 1;
    pub const INT32: u8 = 2;
    pub const FLOAT32: u8 = 3;
    pub const POINTER: u8 = 4;
    pub const COLOR: u8 = 6;
    pub const UINT64: u8 = 7;
    pub const END: u8 = 8;
    pub const INT64: u8 = 10;
    pub const END_ALT: u8 = 11;
}

/// Binary KeyValues `RP` section.
pub fn encode_kv(pairs: &[(String, String)]) -> Vec<u8> {
    let mut out = vec![kv::SECTION];
    out.extend_from_slice(RP_SECTION.as_bytes());
    out.push(0);
    for (key, value) in pairs {
        out.push(kv::STRING);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        out.extend_from_slice(value.as_bytes());
        out.push(0);
    }
    out.push(kv::END);
    out.push(kv::END);
    out
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let (&b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(b)
    }

    fn cstr(&mut self) -> Option<String> {
        let end = self.0.iter().position(|&b| b == 0)?;
        let text = String::from_utf8_lossy(&self.0[..end]).into_owned();
        self.0 = &self.0[end + 1..];
        Some(text)
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }
}

/// Leaves of one section, flattened.
fn read_section(r: &mut Reader, out: &mut Vec<(String, String)>, depth: usize) -> Option<()> {
    if depth > 16 {
        return None;
    }
    loop {
        let kind = r.byte()?;
        if kind == kv::END || kind == kv::END_ALT {
            return Some(());
        }
        let name = r.cstr()?;
        let value = match kind {
            kv::SECTION => {
                read_section(r, out, depth + 1)?;
                continue;
            }
            kv::STRING => r.cstr()?,
            kv::INT32 | kv::POINTER | kv::COLOR => {
                i32::from_le_bytes(r.take(4)?.try_into().ok()?).to_string()
            }
            kv::FLOAT32 => f32::from_le_bytes(r.take(4)?.try_into().ok()?).to_string(),
            kv::UINT64 => u64::from_le_bytes(r.take(8)?.try_into().ok()?).to_string(),
            kv::INT64 => i64::from_le_bytes(r.take(8)?.try_into().ok()?).to_string(),
            _ => return None,
        };
        out.push((name, value));
    }
}

/// Key/value pairs; None when malformed.
pub fn decode_kv(bytes: &[u8]) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    if bytes.is_empty() {
        return Some(out);
    }
    let mut r = Reader(bytes);
    if r.byte()? != kv::SECTION {
        return None;
    }
    r.cstr()?;
    read_section(&mut r, &mut out, 0)?;
    Some(out)
}

/// Message sent with a routing app.
#[derive(Debug)]
struct AppRouted<M> {
    msg: M,
    app_id: u32,
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
