//! Wine prefix health checks.

use crate::launch::context::*;
use crate::launch::pipeline::LaunchPipeline;
use std::collections::HashMap;

impl LaunchPipeline {
    pub(crate) fn check_prefix_health(&self, ctx: &mut PipelineContext) {
        if let Some(spec) = &ctx.command_spec {
            if let Some(prefix) = spec.env.get("WINEPREFIX") {
                let prefix_path = std::path::PathBuf::from(prefix);
                if !prefix_path.exists() {
                    ctx.add_warning(
                        "PREFIX_MISSING",
                        format!("WINEPREFIX does not exist on disk: {}", prefix),
                    );
                    return;
                }

                let mut metadata = HashMap::new();
                metadata.insert("prefix_path".to_string(), prefix.clone());

                // Detect Windows username
                let users_dir = prefix_path.join("drive_c/users");
                if users_dir.exists() {
                    if let Ok(entries) = std::fs::read_dir(&users_dir) {
                        let mut usernames = Vec::new();
                        for entry in entries.flatten() {
                            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                                let name = entry.file_name().to_string_lossy().to_string();
                                if name != "Public" && name != "All Users" && name != "Default User"
                                {
                                    usernames.push(name);
                                }
                            }
                        }
                        // Pick the alphabetically first (most likely primary)
                        if let Some(primary) = usernames.into_iter().min() {
                            ctx.verification.windows_user_path =
                                Some(format!("C:\\users\\{}", primary));
                            metadata
                                .insert("detected_windows_username".to_string(), primary.clone());
                            ctx.verification.windows_username = Some(primary);
                        }
                    }
                }

                let username = ctx
                    .verification
                    .windows_username
                    .as_deref()
                    .unwrap_or("steamuser");
                let common_dirs = [
                    format!("drive_c/users/{}/Documents", username),
                    format!("drive_c/users/{}/AppData/Local", username),
                    format!("drive_c/users/{}/AppData/Roaming", username),
                    "drive_c/Program Files (x86)/Steam/steam.exe".to_string(),
                    "drive_c/Program Files (x86)/Steam/steamclient.dll".to_string(),
                    "drive_c/Program Files (x86)/Steam/tier0_s.dll".to_string(),
                    "drive_c/windows/system32/PhysXLoader.dll".to_string(),
                    "drive_c/windows/syswow64/PhysXLoader.dll".to_string(),
                    "drive_c/windows/system32/steam_api.dll".to_string(),
                    "drive_c/windows/syswow64/steam_api.dll".to_string(),
                    "drive_c/windows/system32/steam_api64.dll".to_string(),
                ];

                for dir in common_dirs {
                    let full_path = prefix_path.join(&dir);
                    let exists = full_path.exists();
                    metadata.insert(format!("path_exists:{}", dir), exists.to_string());
                    ctx.verification.key_paths_detected.insert(dir, exists);
                }

                // Title-Specific Dependency Detection
                let families = &mut ctx.verification.dependency_families_detected;
                let families_to_check = [
                    (
                        "Batman (PhysX/APEX)",
                        vec![
                            "drive_c/windows/system32/PhysXLoader.dll",
                            "drive_c/windows/syswow64/PhysXLoader.dll",
                        ],
                    ),
                    (
                        "Amnesia (SDL2/Newton)",
                        vec![
                            "drive_c/windows/system32/SDL2.dll",
                            "drive_c/windows/syswow64/SDL2.dll",
                        ],
                    ),
                    (
                        "Metro (4A Engine)",
                        vec![
                            "drive_c/windows/system32/PhysXDevice.dll",
                            "drive_c/windows/syswow64/PhysXDevice.dll",
                        ],
                    ),
                ];

                for (family, paths) in families_to_check {
                    if paths.iter().any(|p| prefix_path.join(p).exists())
                        && !families.contains(&family.to_string())
                    {
                        families.push(family.to_string());
                    }
                }

                // Check steam client exposure
                ctx.verification.steam_client_exposed =
                    spec.env.contains_key("STEAM_COMPAT_CLIENT_INSTALL_PATH")
                        || spec
                            .env
                            .get("WINEPATH")
                            .map(|wp| wp.contains("Steam"))
                            .unwrap_or(false);

                ctx.log_info(
                    "prefix_health_check",
                    "WINEPREFIX sanity check complete".to_string(),
                    None,
                    metadata,
                );
            }
        }
    }
}
