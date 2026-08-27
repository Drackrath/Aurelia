use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError, LaunchErrorKind};

pub struct PreparePrefixStage;

#[async_trait]
impl PipelineStage for PreparePrefixStage {
    fn name(&self) -> &str { "PreparePrefix" }
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        let use_symlinks = ctx.user_config.as_ref()
            .is_some_and(|c| c.graphics_layers.use_symlinks_in_prefix);

        if ctx.runner.is_none() {
            return Ok(());
        }

        let runner_ctx = ctx.to_runner_context()?;
        let Some(runner) = &ctx.runner else { return Ok(()) };
        runner.prepare_prefix(&runner_ctx).await?;

        // Post-runner prefix preparation: handle symlinks
        let app_id = runner_ctx.app.app_id;
        let prefix_path = crate::core::utils::wineprefix_for_game(
            &runner_ctx.launcher_config,
            app_id,
            ctx.user_config.as_ref(),
        );

        if !use_symlinks {
            // Cleanup if it was previously enabled
            let _ = crate::core::utils::cleanup_dll_symlinks(&prefix_path);
            return Ok(());
        }

        tracing::info!("Symlink mode enabled, deploying DLLs to prefix: {}", prefix_path.display());
        let deployed = crate::core::utils::deploy_dll_symlinks(&prefix_path, &ctx.dll_resolutions, &ctx.target_architecture)
            .map_err(|e| LaunchError::new(LaunchErrorKind::Permission, format!("failed to deploy symlinks into prefix: {}", e)).with_source(e))?;

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("prefix".into(), prefix_path.to_string_lossy().to_string());
        metadata.insert("deployed_count".into(), deployed.len().to_string());
        ctx.log_info("symlinks_deployed", format!("Deployed {} DLL symlinks into prefix", deployed.len()), Some("PreparePrefix".into()), metadata);
        Ok(())
    }
}
