//! Steam Cloud facts for stand-in games.
//!
//! Files live in the `remote` folder.
//! Launch sync moves them to Steam.

use std::path::PathBuf;

use anyhow::Result;

use crate::core::config::LauncherConfig;
use crate::library::cloud_sync::default_cloud_root;

/// What a game may do with Cloud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStorageInfo {
    /// Cloud sync on for this account.
    pub enabled: bool,
    /// Game writes allowed (opt-in).
    pub writable: bool,
    /// Local mirror of the app's files.
    pub remote_dir: PathBuf,
}

/// Info for `app_id` under `steam_id`.
pub fn remote_storage_info(
    config: &LauncherConfig,
    steam_id: u64,
    app_id: u32,
) -> Result<RemoteStorageInfo> {
    let remote_dir = default_cloud_root(steam_id, app_id)?.join("remote");
    let enabled = config.enable_cloud_sync;
    Ok(RemoteStorageInfo {
        enabled,
        // Writes need sync and opt-in.
        writable: enabled && config.steamworks_cloud_write,
        remote_dir,
    })
}

/// Stand-in games may add Cloud files.
pub fn stand_in_cloud_writes(config: &LauncherConfig) -> bool {
    config.steamworks_client && config.enable_cloud_sync && config.steamworks_cloud_write
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_are_off_by_default() {
        let config = LauncherConfig::default();
        let info = remote_storage_info(&config, 76_561_197_960_287_930, 480).unwrap();
        assert!(info.enabled);
        assert!(!info.writable);
        assert!(info.remote_dir.ends_with("userdata/22202/480/remote"));
    }

    #[test]
    fn uploads_need_stand_in_and_switch() {
        let mut config = LauncherConfig::default();
        assert!(!stand_in_cloud_writes(&config));
        config.steamworks_client = true;
        assert!(!stand_in_cloud_writes(&config));
        config.steamworks_cloud_write = true;
        assert!(stand_in_cloud_writes(&config));
    }

    #[test]
    fn writes_need_cloud_sync_too() {
        let mut config = LauncherConfig {
            steamworks_cloud_write: true,
            ..LauncherConfig::default()
        };
        assert!(remote_storage_info(&config, 1, 1).unwrap().writable);
        config.enable_cloud_sync = false;
        let info = remote_storage_info(&config, 1, 1).unwrap();
        assert!(!info.enabled && !info.writable);
    }
}
