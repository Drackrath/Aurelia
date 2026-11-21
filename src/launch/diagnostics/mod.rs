//! Launch health and duplicate detection.

use crate::launch::context::*;
use crate::launch::pipeline::LaunchPipeline;
use std::collections::HashMap;

mod graphics;
mod prefix;

pub fn detect_duplicate_instance(ctx: &PipelineContext) -> DuplicateInstanceInfo {
    if let Some(spec) = &ctx.command_spec {
        // 1. Check for explicit lockfile in the game directory or prefix
        if let Some(cwd) = &spec.cwd {
            if cwd.join(".aurelia_launch.lock").exists() {
                return DuplicateInstanceInfo {
                    detected: true,
                    source: "lockfile".to_string(),
                };
            }
        }

        // 2. Check for tracked PID if we had a mechanism to store it
        // For now, check if steam.pid exists in the prefix (if applicable)
        if let Some(prefix) = spec.env.get("WINEPREFIX") {
            if std::path::Path::new(prefix).join("steam.pid").exists() {
                // Note: Ideally we'd check if the PID is still alive,
                // but existence of the file in WINEPREFIX is a strong hint.
                return DuplicateInstanceInfo {
                    detected: true,
                    source: "pid".to_string(),
                };
            }
        }
    }

    DuplicateInstanceInfo {
        detected: false,
        source: "none".to_string(),
    }
}

impl LaunchPipeline {
    pub(crate) async fn verify_launch_health(&self, ctx: &mut PipelineContext) {
        if let Some(child) = &mut ctx.child {
            let start_wait = std::time::Instant::now();
            let verify_duration = std::time::Duration::from_millis(2000);

            // Initial wait
            tokio::time::sleep(verify_duration).await;

            match child.try_wait() {
                Ok(Some(status)) => {
                    // Process exited already
                    ctx.verification.status = "failed_after_spawn".to_string();
                    ctx.verification.process_lifetime_ms =
                        Some(start_wait.elapsed().as_millis() as u64);
                    ctx.verification.exit_code = status.code();
                }
                Ok(None) => {
                    // Process still running
                    ctx.verification.status = "verified".to_string();
                    ctx.verification.process_lifetime_ms =
                        Some(start_wait.elapsed().as_millis() as u64);
                }
                Err(e) => {
                    ctx.verification.status = "uncertain".to_string();
                    ctx.log_error(
                        "verification_error",
                        format!("Failed to poll process status: {}", e),
                        None,
                        HashMap::new(),
                    );
                }
            }
        } else {
            ctx.verification.status = "not_verified".to_string();
        }
    }

    pub(crate) fn classify_early_exit(&self, ctx: &mut PipelineContext) {
        if ctx.verification.status != "failed_after_spawn" {
            if ctx.verification.steam_runtime_milestone == "steam_process_exited_early"
                || ctx.verification.steam_auto_start_failed
            {
                ctx.verification.detailed_status = Some("steam_runtime_startup_failed".to_string());
                return;
            }

            if ctx.command_spec.is_none() {
                ctx.verification.detailed_status = Some("command_spec_missing".to_string());
                return;
            }

            if !ctx.executable_exists && ctx.resolved_executable_path.is_some() {
                ctx.verification.detailed_status = Some("game_executable_not_found".to_string());
                return;
            }

            return;
        }

        let dxvk_found = ctx.graphics_stack.runtime_evidence.dxvk.evidence_found;
        let vkd3d_found = ctx
            .graphics_stack
            .runtime_evidence
            .vkd3d_proton
            .evidence_found
            || ctx.graphics_stack.runtime_evidence.vkd3d.evidence_found;

        let mut fatal_error = None;
        for evidence in &ctx.graphics_stack.graphics_stack_evidence {
            if evidence.contains("Override Policy Conflict") {
                fatal_error = Some("override_policy_regression");
                break;
            }
            if evidence.contains("Steam Client/Environment Failure") {
                fatal_error = Some("steam_runtime_exposure_regression");
                break;
            }
            if evidence.contains("PhysX/Middleware failure") {
                fatal_error = Some("middleware_dependency_failure");
                break;
            }
            if evidence.contains("SteamAPI Initialization Failed")
                || evidence.contains("SteamAPI Access Violation")
            {
                fatal_error = Some("steamapi_init_failed");
                break;
            }
            if evidence.contains("Steam Ownership Validation Failed") {
                fatal_error = Some("steam_ownership_or_session_failed");
                break;
            }
            if evidence.contains("SteamAPI Connection Failed") {
                fatal_error = Some("steam_not_found_by_game");
                break;
            }
            if evidence.contains("DLL Load Failure") {
                if evidence.contains("d3d11") {
                    fatal_error = Some("startup_environment_regression");
                } else {
                    fatal_error = Some("missing_required_module");
                }
                break;
            }
            if evidence.contains("DLL Dependency Missing") {
                fatal_error = Some("middleware_or_runtime_component_failure");
                break;
            }
        }

        ctx.verification.detailed_status = Some(if dxvk_found || vkd3d_found {
            "game_failed_after_spawn".to_string()
        } else if let Some(err) = fatal_error {
            err.to_string()
        } else if ctx.verification.steam_running_before_launch
            && ctx.verification.steam_client_exposed
        {
            "steam_sensitive_game_failed_after_spawn".to_string()
        } else {
            "game_failed_after_spawn".to_string()
        });
    }
}

#[cfg(test)]
mod tests;
