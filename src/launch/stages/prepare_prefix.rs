use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError, LaunchErrorKind};

pub struct PreparePrefixStage;

#[async_trait]
impl PipelineStage for PreparePrefixStage {
    fn name(&self) -> &str { "PreparePrefix" }
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        use crate::infra::runners::LaunchContext;

        let use_symlinks = ctx.user_config.as_ref()
            .is_some_and(|c| c.graphics_layers.use_symlinks_in_prefix);

        let Some(runner) = &ctx.runner else { return Ok(()) };

        let missing = |field| LaunchError::new(LaunchErrorKind::Validation, field);
        let runner_ctx = LaunchContext {
            app: ctx.app.as_ref().ok_or_else(|| missing("app missing"))?.clone(),
            launch_info: ctx.launch_info.as_ref().ok_or_else(|| missing("launch_info missing"))?.clone(),
            launcher_config: ctx.launcher_config.as_ref().ok_or_else(|| missing("launcher_config missing"))?.clone(),
            user_config: ctx.user_config.clone(),
            proton_path: ctx.proton_path.clone(),
            steam_enabled: ctx.steam_enabled,
            target_architecture: ctx.target_architecture,
            dll_resolutions: ctx.dll_resolutions.clone(),
            verification_ptr: &mut ctx.verification as *mut _,
        };
        runner.prepare_prefix(&runner_ctx).await?;

        // Under umu, Proton/protonfixes own DLL deployment; skip the in-prefix DLL-symlink
        // step (Aurelia deployed/resolved nothing — see ResolveComponents).
        if ctx.skip_dll_management {
            return Ok(());
        }

        // Post-runner prefix preparation: handle symlinks
        let app_id = runner_ctx.app.app_id;
        let user_configs = ctx.user_config.iter()
            .map(|c| (app_id, c.clone()))
            .collect();
        let prefix_path = crate::utils::steam_wineprefix_for_game(
            &runner_ctx.launcher_config,
            app_id,
            &user_configs,
        );

        if !use_symlinks {
            // Cleanup if it was previously enabled
            let _ = crate::utils::cleanup_dll_symlinks(&prefix_path);
            return Ok(());
        }

        tracing::info!("Symlink mode enabled, deploying DLLs to prefix: {}", prefix_path.display());
        let deployed = crate::utils::deploy_dll_symlinks(&prefix_path, &ctx.dll_resolutions, &ctx.target_architecture)
            .map_err(|e| LaunchError::new(LaunchErrorKind::Permission, format!("failed to deploy symlinks into prefix: {}", e)).with_source(e))?;

        if let Some(logger) = &ctx.logger {
            let mut metadata = std::collections::HashMap::new();
            metadata.insert("prefix".into(), prefix_path.to_string_lossy().to_string());
            metadata.insert("deployed_count".into(), deployed.len().to_string());
            let _ = logger.info("symlinks_deployed", format!("Deployed {} DLL symlinks into prefix", deployed.len()), Some("PreparePrefix".into()), metadata);
        }
        Ok(())
    }
}
