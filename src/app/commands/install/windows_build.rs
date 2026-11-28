//! Keep Steam on a Windows build.

use super::*;
use aurelia::compat::mapping::{self, MappingWrite};

/// Steam Play mapping still to write.
pub(super) struct PendingMapping {
    root: PathBuf,
    tool: String,
}

/// Whether Steam would swap to Linux.
pub(super) fn steam_prefers_linux(
    host: DepotPlatform,
    platform: DepotPlatform,
    platforms: &[DepotPlatform],
    is_dlc: bool,
) -> bool {
    host == DepotPlatform::Linux
        && !is_dlc
        && platform == DepotPlatform::Windows
        && platforms.contains(&DepotPlatform::Linux)
}

/// Plan mapping; Steam must be stopped.
pub(super) fn plan_mapping(app_id: u32, restart_steam: bool) -> Result<Option<PendingMapping>> {
    let Some(root) = aurelia::core::config::detect_steam_path() else {
        return Ok(None);
    };
    if !root.join("config").join("config.vdf").is_file() {
        return Ok(None);
    }
    let mapped = mapping::steam_tool_mappings(&root)
        .iter()
        .any(|m| m.app_id == u64::from(app_id));
    if mapped {
        return Ok(None);
    }
    if SteamClient::steam_is_running() && !restart_steam {
        bail!(
            "Steam is running and would swap app {app_id}'s Windows build for its Linux \
             build. Close Steam first, or re-run with --restart-steam so Aurelia can set \
             a Steam Play tool for it."
        );
    }
    let tool = mapping::suggest_tool(&mapping::installed_tools(&root));
    Ok(Some(PendingMapping { root, tool }))
}

/// Write the mapping; failures only warn.
pub(super) fn apply_mapping(pending: &PendingMapping, app_id: u32, json: bool) -> Option<String> {
    match mapping::ensure_app_mapping(&pending.root, app_id, &pending.tool) {
        Ok(MappingWrite::Added) => {
            if !json {
                cli_println!(
                    "Set Steam Play tool {} for {app_id} so Steam keeps the Windows build.",
                    pending.tool
                );
            }
            Some(pending.tool.clone())
        }
        Ok(MappingWrite::Existing(_)) => None,
        Err(err) => {
            tracing::warn!("Steam Play tool not set for {app_id}: {err:#}");
            if !json {
                cli_eprintln!(
                    "⚠ Could not set a Steam Play tool for {app_id} ({err:#}); Steam may \
                     switch it to the Linux build."
                );
            }
            None
        }
    }
}
