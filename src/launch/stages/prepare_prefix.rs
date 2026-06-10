use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError, LaunchErrorKind};

pub struct PreparePrefixStage;

#[async_trait]
impl PipelineStage for PreparePrefixStage {
    fn name(&self) -> &str { "PreparePrefix" }
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        use crate::infra::runners::LaunchContext;

        let user_cfg = ctx.user_config.as_ref().cloned().unwrap_or_default();
        let use_symlinks = user_cfg.graphics_layers.use_symlinks_in_prefix;

        if let Some(runner) = &ctx.runner {
            let runner_ctx = LaunchContext {
                app: ctx.app.as_ref()
                    .ok_or_else(|| LaunchError::new(LaunchErrorKind::Validation, "app missing"))?.clone(),
                launch_info: ctx.launch_info.as_ref()
                    .ok_or_else(|| LaunchError::new(LaunchErrorKind::Validation, "launch_info missing"))?.clone(),
                launcher_config: ctx.launcher_config.as_ref()
                    .ok_or_else(|| LaunchError::new(LaunchErrorKind::Validation, "launcher_config missing"))?.clone(),
                user_config: ctx.user_config.clone(),
                proton_path: ctx.proton_path.clone(),
                target_architecture: ctx.target_architecture,
                dll_resolutions: ctx.dll_resolutions.clone(),
                verification_ptr: &mut ctx.verification as *mut _,
            };
            runner.prepare_prefix(&runner_ctx).await?;

            // Post-runner prefix preparation: handle symlinks
            let prefix_path = crate::utils::steam_wineprefix_for_game(
                &runner_ctx.launcher_config,
                runner_ctx.app.app_id,
                &ctx.user_config.as_ref().map(|c| {
                    let mut store = std::collections::HashMap::new();
                    store.insert(runner_ctx.app.app_id, c.clone());
                    store
                }).unwrap_or_default().into()
            );

            if use_symlinks {
                tracing::info!("Symlink mode enabled, deploying DLLs to prefix: {}", prefix_path.display());
                match crate::utils::deploy_dll_symlinks(&prefix_path, &ctx.dll_resolutions, &ctx.target_architecture) {
                    Ok(deployed) => {
                        if let Some(logger) = &ctx.logger {
                            let mut metadata = std::collections::HashMap::new();
                            metadata.insert("prefix".into(), prefix_path.to_string_lossy().to_string());
                            metadata.insert("deployed_count".into(), deployed.len().to_string());
                            let _ = logger.info("symlinks_deployed", format!("Deployed {} DLL symlinks into prefix", deployed.len()), Some("PreparePrefix".into()), metadata);
                        }
                    }
                    Err(e) => {
                         return Err(LaunchError::new(LaunchErrorKind::Permission, format!("failed to deploy symlinks into prefix: {}", e)).with_source(e));
                    }
                }
            } else {
                // Cleanup if it was previously enabled
                let _ = crate::utils::cleanup_dll_symlinks(&prefix_path);
            }
        }
        Ok(())
    }
}
