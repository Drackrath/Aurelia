use crate::launch::pipeline::{LaunchError, LaunchErrorKind, PipelineContext, PipelineStage};
use async_trait::async_trait;

pub struct SpawnProcessStage;

#[async_trait]
impl PipelineStage for SpawnProcessStage {
    fn name(&self) -> &str {
        "SpawnProcess"
    }
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        // Native captures game output into session logs.
        if ctx.runner.as_ref().is_some_and(|r| r.name() == "Native") {
            if let (Some(session), Some(spec)) = (&ctx.session, ctx.command_spec.as_mut()) {
                spec.env.insert(
                    "AURELIA_STDOUT_LOG".to_string(),
                    session.stdout_path().to_string_lossy().into_owned(),
                );
                spec.env.insert(
                    "AURELIA_STDERR_LOG".to_string(),
                    session.stderr_path().to_string_lossy().into_owned(),
                );
            }
        }

        #[cfg(target_os = "linux")]
        apply_steamworks_overlay(ctx).await?;

        if let Some(runner) = &ctx.runner {
            if let Some(spec) = &ctx.command_spec {
                // Stringify env only when logging.
                if ctx.logger.is_some() {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert(
                        "program".to_string(),
                        spec.program.to_string_lossy().to_string(),
                    );
                    metadata.insert("args".to_string(), spec.args.join(" "));
                    for (k, v) in &spec.env {
                        metadata.insert(format!("env:{}", k), v.clone());
                    }
                    ctx.log_info(
                        "process_spawn_attempt",
                        "Attempting to spawn process via runner".to_string(),
                        Some("SpawnProcess".to_string()),
                        metadata,
                    );
                }

                let child = runner.launch(spec).map_err(|e| {
                    match e
                        .source
                        .as_ref()
                        .and_then(|s| s.downcast_ref::<std::io::Error>())
                    {
                        Some(io_err) => {
                            let dup_info = crate::launch::pipeline::detect_duplicate_instance(ctx);
                            crate::launch::pipeline::map_io_error(io_err, Some(&dup_info))
                        }
                        None => e,
                    }
                })?;
                // Record immediately; play_game refines it later.
                let _ = crate::compat::running::record_launch(
                    &crate::compat::running::RunningGame::new(
                        ctx.app_id,
                        ctx.app.as_ref().map(|a| a.name.clone()).unwrap_or_default(),
                        child.id(),
                        None,
                    ),
                );
                ctx.child = Some(child);
            }
        } else {
            // Legacy fallback when no runner resolved.
            let app = ctx
                .app
                .as_ref()
                .ok_or_else(|| LaunchError::new(LaunchErrorKind::Validation, "app missing"))?;
            let launch_info = ctx.launch_info.as_ref().ok_or_else(|| {
                LaunchError::new(LaunchErrorKind::Validation, "launch_info missing")
            })?;
            let launcher_config = ctx.launcher_config.as_ref().ok_or_else(|| {
                LaunchError::new(LaunchErrorKind::Validation, "launcher_config missing")
            })?;

            // ResolveComponentsStage normally always sets one.
            let client = crate::steam_client::SteamClient::new().map_err(|e| {
                LaunchError::new(
                    LaunchErrorKind::Environment,
                    "failed to initialize steam client",
                )
                .with_source(e)
            })?;
            let child = client
                .internal_legacy_launch_adhoc(
                    app,
                    launch_info,
                    ctx.proton_path.as_deref(),
                    launcher_config,
                    ctx.user_config.as_ref(),
                )
                .await
                .map_err(|e| {
                    if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                        let dup_info = crate::launch::pipeline::detect_duplicate_instance(ctx);
                        crate::launch::pipeline::map_io_error(io_err, Some(&dup_info))
                    } else {
                        LaunchError::new(LaunchErrorKind::Process, "failed to spawn adhoc process")
                            .with_source(e)
                    }
                })?;
            ctx.child = Some(child);
        }
        Ok(())
    }
}

/// Wrap the command in the overlay.
#[cfg(target_os = "linux")]
async fn apply_steamworks_overlay(ctx: &mut PipelineContext) -> Result<(), LaunchError> {
    use crate::launch::steamworks_overlay as overlay;
    let Some(config) = ctx.launcher_config.as_ref() else {
        return Ok(());
    };
    if !config.steamworks_client {
        return Ok(());
    }
    let runner = ctx.runner.as_ref().map(|r| r.name().to_string());
    let (Some(runner), Some(spec)) = (runner, ctx.command_spec.as_ref()) else {
        tracing::warn!("steamworks_client is on, but this launch path has no overlay");
        return Ok(());
    };
    if !overlay::runner_supported(&runner) {
        tracing::warn!("steamworks_client is on, but runner {runner} has no overlay");
        return Ok(());
    }
    let account_id = crate::core::config::load_session()
        .await
        .ok()
        .and_then(|s| s.steam_id)
        .map(|id| (id & 0xffff_ffff) as u32);
    let plan = overlay::plan_for(config, spec, ctx.app_id, account_id)?;
    tracing::info!(
        sdk64 = %plan.libs.sdk64.display(),
        "launching with the stand-in steamclient.so"
    );
    ctx.command_spec = Some(overlay::wrap_command(spec, &plan));
    Ok(())
}
