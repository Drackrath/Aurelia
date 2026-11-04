//! Replay Vulkan pipelines before launch.

use crate::launch::pipeline::{LaunchError, PipelineContext, PipelineStage};
use crate::launch::shaders::{self, ShaderEvent, ShaderPrecache};
use async_trait::async_trait;

pub struct PrecacheShadersStage;

#[async_trait]
impl PipelineStage for PrecacheShadersStage {
    fn name(&self) -> &str {
        "PrecacheShaders"
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        if !cfg!(target_os = "linux") {
            return Ok(());
        }
        let mode = ctx
            .launcher_config
            .as_ref()
            .map(|c| c.shader_precache)
            .unwrap_or_default();
        if mode == ShaderPrecache::Off {
            return Ok(());
        }
        let Some(dir) = ctx
            .resolved_install_dir
            .as_deref()
            .and_then(|d| shaders::shader_cache_dir(d, ctx.app_id))
        else {
            return Ok(());
        };
        let Some(spec) = ctx.command_spec.as_mut() else {
            return Ok(());
        };

        // Same caches as Steam; user values win.
        if let Err(e) = shaders::ensure_cache_dirs(&dir) {
            tracing::warn!("shader cache folders not created ({e}); using driver defaults");
            return Ok(());
        }
        for (key, value) in shaders::cache_env(&dir) {
            if !spec.env.contains_key(key) && std::env::var_os(key).is_none() {
                spec.env.insert(key.to_string(), value);
            }
        }

        if ctx.skip_shaders {
            emit(ctx, ShaderEvent::Skipped("--skip-shaders".into()));
            return Ok(());
        }
        let env = spec.env.clone();
        let outcome = shaders::precache(&dir, &env, false, ctx.on_shader_event.as_ref()).await;
        match outcome {
            Ok(ShaderEvent::Skipped(reason)) => {
                tracing::info!(app_id = ctx.app_id, "shader pre-caching skipped: {reason}");
                emit(ctx, ShaderEvent::Skipped(reason));
            }
            Ok(_) => tracing::info!(app_id = ctx.app_id, "Vulkan shaders processed"),
            // Shaders then compile in-game.
            Err(e) => tracing::warn!(
                app_id = ctx.app_id,
                "shader pre-caching failed, launching anyway: {e:#}"
            ),
        }
        Ok(())
    }
}

fn emit(ctx: &PipelineContext, event: ShaderEvent) {
    if let Some(cb) = &ctx.on_shader_event {
        cb(event);
    }
}

#[cfg(test)]
#[path = "precache_shaders_tests.rs"]
mod tests;
