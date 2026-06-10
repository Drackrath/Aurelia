use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError, LaunchErrorKind};

pub struct SpawnProcessStage;

#[async_trait]
impl PipelineStage for SpawnProcessStage {
    fn name(&self) -> &str { "SpawnProcess" }
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        if let Some(runner) = &ctx.runner {
            if let Some(spec) = &ctx.command_spec {
                if let Some(logger) = &ctx.logger {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert("program".to_string(), spec.program.to_string_lossy().to_string());
                    metadata.insert("args".to_string(), spec.args.join(" "));
                    for (k, v) in &spec.env {
                        metadata.insert(format!("env:{}", k), v.clone());
                    }
                    let _ = logger.info("process_spawn_attempt", "Attempting to spawn process via runner".to_string(), Some("SpawnProcess".to_string()), metadata);
                }

                let child = runner.launch(spec).map_err(|e| {
                    if let Some(source) = &e.source {
                        if let Some(io_err) = source.downcast_ref::<std::io::Error>() {
                            let dup_info = crate::launch::pipeline::detect_duplicate_instance(ctx);
                            return crate::launch::pipeline::map_io_error(io_err, Some(&dup_info));
                        }
                    }
                    e
                })?;
                ctx.child = Some(child);
            }
        } else {
            // Legacy/Native fallback if no runner resolved
            let app = ctx.app.as_ref().ok_or_else(|| LaunchError::new(LaunchErrorKind::Validation, "app missing"))?;
            let launch_info = ctx.launch_info.as_ref().ok_or_else(|| LaunchError::new(LaunchErrorKind::Validation, "launch_info missing"))?;
            let launcher_config = ctx.launcher_config.as_ref().ok_or_else(|| LaunchError::new(LaunchErrorKind::Validation, "launcher_config missing"))?;

            // For now, we use the legacy path for NativeLinux until NativeRunner is implemented
            let client = crate::steam_client::SteamClient::new()
                .map_err(|e| LaunchError::new(LaunchErrorKind::Environment, "failed to initialize steam client").with_source(e))?;
            let child = client.internal_legacy_launch_adhoc(
                app,
                launch_info,
                ctx.proton_path.as_deref(),
                launcher_config,
                ctx.user_config.as_ref()
            ).await.map_err(|e| {
                if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                    let dup_info = crate::launch::pipeline::detect_duplicate_instance(ctx);
                    crate::launch::pipeline::map_io_error(io_err, Some(&dup_info))
                } else {
                    LaunchError::new(LaunchErrorKind::Process, "failed to spawn adhoc process").with_source(e)
                }
            })?;
            ctx.child = Some(child);
        }
        Ok(())
    }
}
