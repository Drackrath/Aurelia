//! Launch summary artifact writer.

use crate::launch::context::*;
use crate::launch::pipeline::LaunchPipeline;
use std::collections::HashMap;

impl LaunchPipeline {
    pub(crate) fn write_summary_if_possible(
        &self,
        ctx: &mut PipelineContext,
        result: crate::logging::LaunchResult,
        failing_stage: Option<String>,
        total_duration_ms: u128,
        stage_durations_ms: HashMap<String, u128>,
    ) {
        let (sanity_warnings, env_snapshot) = if let Some(spec) = &ctx.command_spec {
            let runner_name = ctx
                .runner
                .as_ref()
                .map(|r| r.name().to_string())
                .unwrap_or_else(|| "Unknown".to_string());

            let warnings = crate::logging::check_environment_sanity(
                &spec.env,
                &runner_name,
                ctx.user_config.as_ref(),
            );

            let env_snapshot = crate::logging::EffectiveEnv {
                runner_name,
                profile_id: ctx.launch_info.as_ref().map(|l| l.id.clone()),
                profile_name: ctx.launch_info.as_ref().map(|l| l.description.clone()),
                wine_dll_overrides: spec.env.get("WINEDLLOVERRIDES").cloned(),
                env_vars: spec.env.clone(),
            };

            (warnings, Some(env_snapshot))
        } else {
            (Vec::new(), None)
        };

        for warning in sanity_warnings {
            ctx.add_warning(warning.code, warning.message);
        }

        if let Some(session) = &ctx.session {
            if let Some(env) = env_snapshot {
                let _ = session.write_effective_env(&env);
                let _ = session.write_effective_env_txt(&env.env_vars);
            }
            if let Some(spec) = &ctx.command_spec {
                let _ = session.write_command_artifact(spec);
            }
            let _ = session.write_dll_resolution_artifact(&ctx.dll_resolutions);

            // Write Effective Launch Config
            if let Some(spec) = &ctx.command_spec {
                use crate::logging::{
                    EffectiveCommandConfig, EffectiveGameConfig, EffectiveLaunchConfig,
                    EffectiveRunnerConfig, EffectiveSettingsConfig,
                };
                let config = EffectiveLaunchConfig {
                    session_id: session.id.to_string(),
                    app_id: ctx.app_id,
                    app_name: ctx.app.as_ref().map(|a| a.name.clone()),
                    game: EffectiveGameConfig {
                        install_dir: ctx.resolved_install_dir.clone(),
                        executable_path: ctx.resolved_executable_path.clone(),
                        executable_exists: ctx.executable_exists,
                    },
                    runner: EffectiveRunnerConfig {
                        name: ctx.runner.as_ref().map(|r| r.name().to_string()),
                        root: {
                            let proton = if let Some(forced) = ctx
                                .launcher_config
                                .as_ref()
                                .and_then(|c| c.game_configs.get(&ctx.app_id))
                                .and_then(|c| c.forced_proton_version.as_ref())
                            {
                                forced.as_str()
                            } else {
                                ctx.proton_path.as_deref().unwrap_or("wine")
                            };
                            let library_root = ctx
                                .launcher_config
                                .as_ref()
                                .map(|c| std::path::PathBuf::from(&c.steam_library_path))
                                .unwrap_or_default();
                            let active_runner =
                                crate::core::runner::resolve_runner(proton, &library_root);
                            Some(crate::core::runner::derive_runner_root(&active_runner))
                        },
                    },
                    settings: EffectiveSettingsConfig {
                        requested_backend: ctx.graphics_stack.requested_backend.clone(),
                        effective_backend: ctx.graphics_stack.effective_backend.clone(),
                        requested_d3d12_provider: ctx
                            .graphics_stack
                            .requested_d3d12_provider
                            .clone(),
                        effective_d3d12_provider: ctx
                            .graphics_stack
                            .effective_d3d12_provider
                            .clone(),
                        requested_nvapi: ctx.graphics_stack.requested_nvapi,
                        effective_nvapi: ctx.graphics_stack.effective_nvapi,
                        requested_gpu: ctx.graphics_stack.requested_gpu.clone(),
                        effective_gpu: ctx.graphics_stack.effective_gpu.clone(),
                        target_architecture: ctx.target_architecture,
                        dll_resolutions: ctx.dll_resolutions.clone(),
                        wine_dll_overrides: spec.env.get("WINEDLLOVERRIDES").cloned(),
                        runtime_evidence: Some(ctx.graphics_stack.runtime_evidence.clone()),
                        env_propagation: Some(ctx.graphics_stack.env_propagation.clone()),
                    },
                    command: EffectiveCommandConfig {
                        program: spec.program.clone(),
                        args: spec.args.clone(),
                        cwd: spec.cwd.clone(),
                        env_subset: {
                            let mut subset = HashMap::new();
                            for key in &[
                                "DRI_PRIME",
                                "__NV_PRIME_RENDER_OFFLOAD",
                                "__NV_PRIME_RENDER_OFFLOAD_PROVIDER",
                                "DXVK_HUD",
                                "VKD3D_DEBUG",
                            ] {
                                if let Some(val) = spec.env.get(*key) {
                                    subset.insert(key.to_string(), val.clone());
                                }
                            }
                            subset
                        },
                    },
                    fallbacks: ctx.graphics_stack.fallback_reasons.clone(),
                };
                let _ = session.write_effective_launch_config(&config);
            }
        }

        if let Some(session) = &ctx.session {
            let timestamp = crate::core::time::now_unix();

            let summary = crate::logging::LaunchSummary {
                session_id: session.id.to_string(),
                app_id: ctx.app_id,
                app_name: ctx.app.as_ref().map(|a| a.name.clone()),
                runner_name: ctx.runner.as_ref().map(|r| r.name().to_string()),
                result,
                failing_stage,
                total_duration_ms,
                stage_durations_ms,
                timestamp,
                warnings: ctx.warnings.clone(),
                graphics_stack: Some(ctx.graphics_stack.clone()),
                verification: ctx.verification.clone(),
            };

            let _ = session.write_summary(&summary);

            // Add concise summary block to events log
            if ctx.logger.is_some() {
                let mut metadata = HashMap::new();
                if let Some(path) = &ctx.resolved_executable_path {
                    metadata.insert("exe".to_string(), path.to_string_lossy().to_string());
                }
                metadata.insert(
                    "backend".to_string(),
                    ctx.graphics_stack.effective_backend.clone(),
                );
                metadata.insert(
                    "d3d12_provider".to_string(),
                    ctx.graphics_stack.effective_d3d12_provider.clone(),
                );
                metadata.insert(
                    "gpu".to_string(),
                    ctx.graphics_stack
                        .effective_gpu
                        .clone()
                        .unwrap_or_else(|| "default".to_string()),
                );
                metadata.insert(
                    "arch".to_string(),
                    format!("{:?}", ctx.target_architecture).to_lowercase(),
                );
                if let Some(spec) = &ctx.command_spec
                    && let Some(overrides) = spec.env.get("WINEDLLOVERRIDES")
                {
                    metadata.insert("overrides".to_string(), overrides.clone());
                }
                metadata.insert(
                    "validation_passed".to_string(),
                    ctx.warnings.is_empty().to_string(),
                );
                metadata.insert(
                    "fallback_occurred".to_string(),
                    (!ctx.graphics_stack.fallback_reasons.is_empty()).to_string(),
                );

                if let Some(spec) = &ctx.command_spec
                    && let Some(prefix) = spec.env.get("WINEPREFIX")
                {
                    metadata.insert("prefix".to_string(), prefix.clone());
                }
                if let Some(config) = &ctx.launcher_config {
                    metadata.insert(
                        "shared_prefix".to_string(),
                        config.use_shared_compat_data.to_string(),
                    );
                }
                metadata.insert(
                    "nvapi_requested".to_string(),
                    ctx.graphics_stack.requested_nvapi.to_string(),
                );
                metadata.insert(
                    "nvapi_exposed".to_string(),
                    ctx.graphics_stack.effective_nvapi.to_string(),
                );
                if let Some(ref detailed) = ctx.verification.detailed_status {
                    metadata.insert("verification_detailed".to_string(), detailed.clone());
                }

                if let Some(ref username) = ctx.verification.windows_username {
                    metadata.insert("windows_user".to_string(), username.clone());
                }
                metadata.insert(
                    "steam_client_exposed".to_string(),
                    ctx.verification.steam_client_exposed.to_string(),
                );
                metadata.insert(
                    "last_milestone".to_string(),
                    ctx.verification.last_successful_startup_milestone.clone(),
                );
                if !ctx.verification.dependency_families_detected.is_empty() {
                    metadata.insert(
                        "dependency_families".to_string(),
                        ctx.verification.dependency_families_detected.join(", "),
                    );
                }
                if !ctx.verification.steam_runtime_milestone.is_empty() {
                    metadata.insert(
                        "steam_runtime_milestone".to_string(),
                        ctx.verification.steam_runtime_milestone.clone(),
                    );
                }
                if let Some(init) = ctx.verification.steam_api_initialized {
                    metadata.insert("steam_api_initialized".to_string(), init.to_string());
                }
                if let Some(own) = ctx.verification.steam_ownership_confirmed {
                    metadata.insert("steam_ownership_confirmed".to_string(), own.to_string());
                }
                if let Some(ref art) = ctx.verification.steam_client_artifact {
                    metadata.insert("steam_client_artifact".to_string(), art.clone());
                }
                if let Some(ref pfx) = ctx.verification.effective_game_wineprefix {
                    metadata.insert("effective_game_wineprefix".to_string(), pfx.clone());
                }
                if let Some(ref pfx) = ctx.verification.effective_steam_wineprefix {
                    metadata.insert("effective_steam_wineprefix".to_string(), pfx.clone());
                }
                if let Some(ref path) = ctx.verification.steam_client_install_path_exposed_to_game {
                    metadata.insert(
                        "steam_client_install_path_exposed_to_game".to_string(),
                        path.clone(),
                    );
                }
                if let Some(ref source) = ctx.verification.steam_client_install_path_source {
                    metadata.insert(
                        "steam_client_install_path_source".to_string(),
                        source.clone(),
                    );
                }
                metadata.insert(
                    "per_game_prefix_requested".to_string(),
                    ctx.verification.per_game_prefix_requested.to_string(),
                );
                metadata.insert(
                    "per_game_prefix_honored".to_string(),
                    ctx.verification.per_game_prefix_honored.to_string(),
                );

                metadata.insert(
                    "steam_running_before_launch".to_string(),
                    ctx.verification.steam_running_before_launch.to_string(),
                );
                metadata.insert(
                    "steam_auto_start_attempted".to_string(),
                    ctx.verification.steam_auto_start_attempted.to_string(),
                );
                metadata.insert(
                    "steam_auto_start_failed".to_string(),
                    ctx.verification.steam_auto_start_failed.to_string(),
                );

                ctx.log_info(
                    "launch_summary_concise",
                    "Concise launch summary recorded".to_string(),
                    None,
                    metadata,
                );
            }
        }
    }
}
