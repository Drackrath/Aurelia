//! Steam's clock for stand-in games.
//!
//! Offset kept from the CM logon.
use super::*;

/// Local Unix time in ms.
pub fn local_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Unix seconds with the offset.
pub fn apply(local_ms: u64, offset_ms: i64) -> u32 {
    let now = i64::try_from(local_ms)
        .unwrap_or(i64::MAX)
        .saturating_add(offset_ms);
    u32::try_from(now.max(0) / 1000).unwrap_or(u32::MAX)
}

impl SteamClient {
    /// Steam's current Unix time.
    ///
    /// No logon time: local clock.
    pub async fn server_time(&self) -> Result<u32> {
        Ok(self.require_connection()?.server_time())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_rounds_to_seconds() {
        assert_eq!(apply(1_800_000_000_500, 0), 1_800_000_000);
        assert_eq!(apply(1_800_000_000_000, 61_000), 1_800_000_061);
        assert_eq!(apply(500, -10_000), 0);
    }

    #[test]
    fn not_logged_in_fails() {
        let client = SteamClient::new().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(rt.block_on(client.server_time()).is_err());
    }
}
