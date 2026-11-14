//! Steam's clock for stand-in games.
//!
//! Asked once per CM session.
use super::*;
use std::collections::HashMap;
use std::sync::Mutex;
use steam_vent_proto::steammessages_clientserver_login::{
    CMsgClientServerTimestampRequest, CMsgClientServerTimestampResponse,
};

/// Offset in ms per session.
static OFFSETS: Mutex<Option<HashMap<i32, i64>>> = Mutex::new(None);

/// Local Unix time in ms.
pub fn local_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Server minus local, midpoint corrected.
pub fn offset_ms(sent_ms: u64, server_ms: u64, received_ms: u64) -> i64 {
    let mid = sent_ms / 2 + received_ms / 2 + (sent_ms % 2 + received_ms % 2) / 2;
    i64::try_from(server_ms).unwrap_or(i64::MAX) - i64::try_from(mid).unwrap_or(i64::MAX)
}

/// Unix seconds with the offset.
pub fn apply(local_ms: u64, offset_ms: i64) -> u32 {
    let now = i64::try_from(local_ms)
        .unwrap_or(i64::MAX)
        .saturating_add(offset_ms);
    u32::try_from(now.max(0) / 1000).unwrap_or(u32::MAX)
}

fn cached(session: i32) -> Option<i64> {
    OFFSETS.lock().unwrap().as_ref()?.get(&session).copied()
}

fn remember(session: i32, offset: i64) {
    OFFSETS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(session, offset);
}

impl SteamClient {
    /// Steam's current Unix time.
    pub async fn server_time(&self) -> Result<u32> {
        let connection = self.require_connection()?;
        let session = connection.session_id();
        if let Some(offset) = cached(session) {
            return Ok(apply(local_ms(), offset));
        }
        let sent = local_ms();
        let mut req = CMsgClientServerTimestampRequest::new();
        req.set_client_request_timestamp(sent);
        let res: CMsgClientServerTimestampResponse = connection
            .job(req)
            .await
            .map_err(|e| anyhow!("Steam server time request failed: {e}"))?;
        let received = local_ms();
        if !res.has_server_timestamp_ms() {
            bail!("Steam sent no server time");
        }
        let offset = offset_ms(sent, res.server_timestamp_ms(), received);
        remember(session, offset);
        Ok(apply(received, offset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_uses_round_trip_midpoint() {
        // Sent 1000, back 1200, server 5100.
        assert_eq!(offset_ms(1_000, 5_100, 1_200), 4_000);
        assert_eq!(offset_ms(1_001, 1_000, 1_002), -1);
    }

    #[test]
    fn apply_rounds_to_seconds() {
        assert_eq!(apply(1_800_000_000_500, 0), 1_800_000_000);
        assert_eq!(apply(1_800_000_000_000, 61_000), 1_800_000_061);
        assert_eq!(apply(500, -10_000), 0);
    }

    #[test]
    fn cache_is_per_session() {
        remember(-77, 2_000);
        assert_eq!(cached(-77), Some(2_000));
        assert_eq!(cached(-78), None);
    }

    #[test]
    fn not_logged_in_fails() {
        let client = SteamClient::new().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(rt.block_on(client.server_time()).is_err());
    }
}
