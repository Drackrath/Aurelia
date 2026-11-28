//! Keep Steam on the installed build.

use super::*;
use aurelia::compat::mapping::{self, MappingWrite};

/// Steam Play change still to write.
pub(super) enum PendingMapping {
    /// Force a tool for Windows builds.
    Add { root: PathBuf, tool: String },
    /// Drop a forced Windows tool.
    Remove { root: PathBuf, tool: String },
}

/// What [`apply_mapping`] changed.
#[derive(Default)]
pub(super) struct AppliedMapping {
    pub(super) added: Option<String>,
    pub(super) removed: Option<String>,
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

/// Whether a native install has a Windows twin.
pub(super) fn native_with_windows_twin(
    host: DepotPlatform,
    platform: DepotPlatform,
    platforms: &[DepotPlatform],
    is_dlc: bool,
) -> bool {
    host == DepotPlatform::Linux
        && !is_dlc
        && platform == DepotPlatform::Linux
        && platforms.contains(&DepotPlatform::Windows)
}

/// Steam root with a config.vdf.
fn steam_config_root() -> Option<PathBuf> {
    aurelia::core::config::detect_steam_path()
        .filter(|root| root.join("config").join("config.vdf").is_file())
}

/// The app's current tool, if any.
fn mapped_tool(root: &std::path::Path, app_id: u32) -> Option<String> {
    mapping::steam_tool_mappings(root)
        .into_iter()
        .find(|m| m.app_id == u64::from(app_id))
        .map(|m| m.tool)
}

/// Plan mapping; Steam must be stopped.
pub(super) fn plan_mapping(app_id: u32, restart_steam: bool) -> Result<Option<PendingMapping>> {
    let Some(root) = steam_config_root() else {
        return Ok(None);
    };
    if mapped_tool(&root, app_id).is_some() {
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
    Ok(Some(PendingMapping::Add { root, tool }))
}

/// Plan unmapping; Steam must be stopped.
pub(super) fn plan_unmapping(app_id: u32, restart_steam: bool) -> Result<Option<PendingMapping>> {
    let Some(root) = steam_config_root() else {
        return Ok(None);
    };
    let Some(tool) = mapped_tool(&root, app_id) else {
        return Ok(None);
    };
    if !mapping::runs_windows_builds(&tool, &mapping::installed_tool_platforms(&root)) {
        return Ok(None);
    }
    if SteamClient::steam_is_running() && !restart_steam {
        bail!(
            "Steam is running and would swap app {app_id}'s native build for its Windows \
             build: Steam Play tool {tool} is forced for it. Close Steam first, or re-run \
             with --restart-steam so Aurelia can remove it."
        );
    }
    Ok(Some(PendingMapping::Remove { root, tool }))
}

/// Write the change; failures only warn.
pub(super) fn apply_mapping(pending: &PendingMapping, app_id: u32, json: bool) -> AppliedMapping {
    let warn = |err: anyhow::Error| {
        tracing::warn!("Steam Play tool not updated for {app_id}: {err:#}");
        if !json {
            cli_eprintln!(
                "⚠ Could not update the Steam Play tool for {app_id} ({err:#}); Steam may \
                 switch it to the other build."
            );
        }
    };
    let mut applied = AppliedMapping::default();
    match pending {
        PendingMapping::Add { root, tool } => match mapping::ensure_app_mapping(root, app_id, tool)
        {
            Ok(MappingWrite::Added) => {
                if !json {
                    cli_println!(
                        "Set Steam Play tool {tool} for {app_id} so Steam keeps the Windows build."
                    );
                }
                applied.added = Some(tool.clone());
            }
            Ok(MappingWrite::Existing(_)) => {}
            Err(err) => warn(err),
        },
        PendingMapping::Remove { root, tool } => {
            match mapping::remove_app_mapping_file(root, app_id) {
                Ok(true) => {
                    if !json {
                        cli_println!(
                            "Removed Steam Play tool {tool} from {app_id} so Steam keeps the \
                             native build."
                        );
                    }
                    applied.removed = Some(tool.clone());
                }
                Ok(false) => {}
                Err(err) => warn(err),
            }
        }
    }
    applied
}
