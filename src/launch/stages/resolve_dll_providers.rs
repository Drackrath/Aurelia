use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError, LaunchErrorKind};
use crate::launch::dll_provider_resolver::DllProviderResolver;
use std::path::PathBuf;

pub struct ResolveDllProvidersStage;

/// Derive the full executable path and its containing directory from the install
/// path and an optional launch-info executable (whose separators are normalized
/// to `/`). When no executable is given, both fall back to the install path.
fn exe_paths_from(install_path: &str, executable: Option<&str>) -> (PathBuf, PathBuf) {
    let install = PathBuf::from(install_path);
    match executable {
        Some(exe) => {
            let exe_path = install.join(exe.replace('\\', "/"));
            // Joining a relative parent is equivalent to taking the parent of the
            // joined path; fall back to the install dir when there is no parent.
            let exe_dir = exe_path.parent().map_or_else(|| install.clone(), PathBuf::from);
            (exe_path, exe_dir)
        }
        None => (install.clone(), install),
    }
}

#[async_trait]
impl PipelineStage for ResolveDllProvidersStage {
    fn name(&self) -> &str { "ResolveDllProviders" }

    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        // Under umu, Proton/protonfixes own DLL deployment + WINEDLLOVERRIDES. Aurelia
        // deploys nothing, so skip DLL resolution entirely (see ResolveComponents).
        if ctx.skip_dll_management {
            tracing::info!("skip_dll_management set (umu runner): bypassing DLL provider resolution");
            return Ok(());
        }

        let app = ctx.app.as_ref().ok_or_else(|| LaunchError::new(LaunchErrorKind::Validation, "App context missing"))?;
        let install_path = app.install_path.as_ref().ok_or_else(|| LaunchError::new(LaunchErrorKind::GameData, "Install path missing"))?;

        // Derive the executable path and its directory once; with launch info we
        // can be more precise about the exe dir than just the install root.
        let executable = ctx.launch_info.as_ref().map(|info| info.executable.as_str());
        let (exe_path, game_exe_dir) = exe_paths_from(install_path, executable);

        let nvapi_enabled = ctx.user_config.as_ref()
            .map_or(true, |c| c.graphics_layers.nvapi_enabled);

        let resolver = DllProviderResolver::new();

        // We need runner components. This implies ResolveComponentsStage must have run.
        // Actually, we can just detect them here or ensure ResolveComponentsStage provides them.
        let proton_path = ctx.proton_path.as_deref().unwrap_or("wine");
        let library_root = ctx.launcher_config.as_ref().map(|c| PathBuf::from(&c.steam_library_path)).unwrap_or_default();
        let resolved_runner = crate::utils::resolve_runner(proton_path, &library_root);

        // Resolve WINEPREFIX for component detection
        let wineprefix = if let (Some(config), Some(app)) = (&ctx.launcher_config, &ctx.app) {
            let mut store = std::collections::HashMap::new();
            if let Some(user_config) = &ctx.user_config {
                store.insert(app.app_id, user_config.clone());
            }
            Some(crate::utils::steam_wineprefix_for_game(config, app.app_id, &store))
        } else {
            None
        };

        let components = crate::utils::detect_runner_components(&resolved_runner, wineprefix.as_deref());
        let d3d12_policy = ctx.user_config.as_ref().map(|c| c.graphics_layers.d3d12_policy.clone()).unwrap_or_default();

        let (custom_dxvk, custom_vkd3d, custom_vkd3d_proton) = if let Some(config) = &ctx.user_config {
            (
                config.graphics_layers.custom_dxvk_path.as_deref(),
                config.graphics_layers.custom_vkd3d_path.as_deref(),
                config.graphics_layers.custom_vkd3d_proton_path.as_deref(),
            )
        } else {
            (None, None, None)
        };

        // Detect architecture before resolution
        if exe_path.exists() {
            ctx.target_architecture = crate::utils::detect_exe_architecture(&exe_path);
            // Pre-populate this so downstream stages can use it
            ctx.resolved_executable_path = Some(exe_path.clone());

            if let Some(logger) = &ctx.logger {
                let mut metadata = std::collections::HashMap::new();
                metadata.insert("exe_path".into(), exe_path.to_string_lossy().to_string());
                metadata.insert("detected_arch".into(), format!("{:?}", ctx.target_architecture).to_lowercase());
                metadata.insert("detection_method".into(), "PE header".to_string());
                let _ = logger.info("arch_detected", "Target executable architecture determined".into(), Some("ResolveDllProviders".into()), metadata);
            }
        }

        let (mut resolutions, scan_report) = resolver.resolve(
            &game_exe_dir,
            &resolved_runner,
            &components,
            &d3d12_policy,
            &ctx.target_architecture,
            custom_dxvk,
            custom_vkd3d,
            custom_vkd3d_proton,
        );

        if !nvapi_enabled {
            for res in &mut resolutions {
                if res.name.contains("nvapi") || res.name.contains("nvofapi") {
                    res.chosen_provider = crate::launch::dll_provider_resolver::DllProvider::None;
                    res.chosen_path = None;
                    res.fallback_reason = Some("NVAPI is disabled in per-game settings".to_string());
                }
            }
        }

        ctx.dll_resolutions = resolutions;

        // Strict Backend Policy Enforcement
        if let Some(config) = &ctx.user_config {
            let backend_policy = &config.graphics_layers.graphics_backend_policy;

            if *backend_policy == crate::models::GraphicsBackendPolicy::DXVK {
                let mut missing_capabilities = Vec::new();

                let has_capability = |name: &str| -> bool {
                    use crate::launch::dll_provider_resolver::DllProvider;
                    ctx.dll_resolutions.iter()
                        .find(|r| r.name == name)
                        .is_some_and(|r| matches!(
                            r.chosen_provider,
                            DllProvider::Runner | DllProvider::GameLocal | DllProvider::Internal
                        ))
                };

                let has_dx11_dxgi = has_capability("d3d11") && has_capability("dxgi");
                let has_dx10_core = has_capability("d3d10core");
                let has_dx9 = has_capability("d3d9");
                let has_dx8 = has_capability("d3d8");

                if !has_dx11_dxgi {
                    missing_capabilities.push("DX11/DXGI (requires d3d11.dll and dxgi.dll)");
                }
                if !has_dx10_core {
                    missing_capabilities.push("DX10/10.1 capability incomplete: missing d3d10core.dll");
                } else if !has_dx11_dxgi {
                    missing_capabilities.push("DX10/10.1 support unavailable because d3d11.dll or dxgi.dll could not be resolved");
                }
                if !has_dx9 {
                    missing_capabilities.push("DX9 (requires d3d9.dll)");
                }
                if !has_dx8 {
                    missing_capabilities.push(if has_dx9 {
                        "DX8 (requires d3d8.dll)"
                    } else {
                        "DX8 (requires d3d8.dll and DX9)"
                    });
                }

                if !missing_capabilities.is_empty() {
                    return Err(LaunchError::new(
                        LaunchErrorKind::Environment,
                        format!("Explicit DXVK mode requested but required capabilities are missing: {}. \
                                Ensure a complete DXVK translation set for architecture {:?} is bundled with your runner or present in the game directory.",
                                missing_capabilities.join("; "), ctx.target_architecture)
                    ).with_context("missing_capabilities", missing_capabilities.join(",")));
                }
            }
        }

        if let Some(session) = &ctx.session {
            let scan_report_path = session.log_dir.join("component_scan.json");
            if let Ok(content) = serde_json::to_string_pretty(&scan_report) {
                let _ = std::fs::write(scan_report_path, content);
            }
        }

        if let Some(logger) = &ctx.logger {
            if scan_report.warnings.is_empty() && scan_report.scan_roots.is_empty() {
                let _ = logger.log(crate::infra::logging::LogLevel::Warn, "zero_runner_roots", "Zero Runner roots derived from runner path".into(), Some("ResolveDllProviders".into()), std::collections::HashMap::new());
            }

            for res in &ctx.dll_resolutions {
                let mut metadata = std::collections::HashMap::new();
                metadata.insert("dll".into(), res.name.clone());
                metadata.insert("provider".into(), format!("{:?}", res.chosen_provider));
                if let Some(path) = &res.chosen_path {
                    metadata.insert("path".into(), path.to_string_lossy().to_string());
                }

                use crate::launch::dll_provider_resolver::DllProvider;
                let count_existing = |provider: DllProvider| {
                    res.candidates.iter().filter(|c| c.provider == provider && c.exists).count()
                };

                metadata.insert("count_gamelocal".into(), count_existing(DllProvider::GameLocal).to_string());
                metadata.insert("count_runner".into(), count_existing(DllProvider::Runner).to_string());
                metadata.insert("count_system".into(), count_existing(DllProvider::System).to_string());

                let _ = logger.info("dll_resolved", format!("Resolved {} to {:?}", res.name, res.chosen_provider), Some("ResolveDllProviders".into()), metadata);
            }
        }

        Ok(())
    }
}
