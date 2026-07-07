use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError};

/// Resolves the data-driven per-game fixups (see [`crate::launch::fixups`]) for the
/// launching app and stashes them on the pipeline context. The runner's `build_env`
/// later merges these env / DLL-override fragments into the effective environment,
/// with explicit user/per-game settings taking precedence.
///
/// Runs after the game/profile are resolved (so `app_id` is known) and before the
/// environment is built.
pub struct ResolveGameFixupsStage;

#[async_trait]
impl PipelineStage for ResolveGameFixupsStage {
    fn name(&self) -> &str { "ResolveGameFixups" }

    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        let app_id = ctx.app.as_ref().map(|a| a.app_id).unwrap_or(ctx.app_id);
        let fixups = crate::launch::fixups::game_fixups(app_id);

        if !fixups.is_empty() {
            tracing::info!(
                app_id,
                env_count = fixups.env.len(),
                dll_count = fixups.dll_overrides.len(),
                "Applying per-game fixups from registry"
            );
            if let Some(logger) = &ctx.logger {
                let mut metadata = std::collections::HashMap::new();
                metadata.insert("app_id".into(), app_id.to_string());
                metadata.insert("env_count".into(), fixups.env.len().to_string());
                metadata.insert("dll_override_count".into(), fixups.dll_overrides.len().to_string());
                let _ = logger.info(
                    "game_fixups_resolved",
                    format!("Resolved {} env + {} DLL-override fixups", fixups.env.len(), fixups.dll_overrides.len()),
                    Some("ResolveGameFixups".into()),
                    metadata,
                );
            }
        }

        ctx.game_fixups = fixups;
        Ok(())
    }
}
