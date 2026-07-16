use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;

use crate::launch::launch_script;
use crate::launch::pipeline::{LaunchError, LaunchErrorKind, PipelineContext, PipelineStage};

/// Wraps the resolved launch command with a per-game launch script when one is
/// active. Runs **between** `PreflightStage` (which validates the real runner
/// binary against the un-wrapped spec) and `SpawnProcessStage` (which spawns
/// whatever `ctx.command_spec` ends up pointing at), so the launch is transparently
/// redirected through the script.
///
/// The transform: with an active script `S` and resolved spec `{ program: P, args:
/// [A0, A1, ...] }`, the new spec becomes `{ program: S, args: [P, A0, A1, ...] }`
/// (cwd preserved). The script receives the resolved command as `"$@"`, so
/// `exec "$@"` is a passthrough. Aurelia also exports `AURELIA_*` env vars alongside
/// the existing launch environment.
pub struct ApplyLaunchScriptStage;

#[async_trait]
impl PipelineStage for ApplyLaunchScriptStage {
    fn name(&self) -> &str { "ApplyLaunchScript" }

    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        let script = launch_script::resolve(
            ctx.app_id,
            ctx.launcher_config.as_ref(),
            ctx.launch_script_override.as_deref(),
            ctx.disable_launch_script,
        );

        let Some(script) = script else { return Ok(()); };

        // No command_spec means the legacy/native fallback path in SpawnProcessStage
        // builds and spawns its own command; there is nothing to wrap here. This is
        // an accepted no-op (the fallback path does not currently support scripts).
        if ctx.command_spec.is_none() {
            return Ok(());
        }

        // Auto-detected dir scripts only reach here when they exist on disk, so a
        // missing file means an explicit `--script` / config `launch_script` path is
        // wrong. Surface it rather than silently spawning the real command.
        if !script.exists() {
            return Err(LaunchError::new(
                LaunchErrorKind::Validation,
                format!("Launch script not found: {}", script.display()),
            )
            .with_context("app_id", ctx.app_id.to_string())
            .with_context("launch_script", script.to_string_lossy()));
        }

        // Gather read-only context before taking the mutable spec borrow.
        let app_id = ctx.app_id;
        let app_name = ctx
            .app
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_default();
        let game_dir = ctx
            .resolved_install_dir
            .clone()
            .or_else(|| {
                ctx.app
                    .as_ref()
                    .and_then(|a| a.install_path.as_ref().map(PathBuf::from))
            });

        let spec = ctx.command_spec.as_mut().expect("command_spec present");
        let old_program = spec.program.clone();
        let old_args = std::mem::take(&mut spec.args);

        // Export AURELIA_* env vars alongside the already-resolved launch env.
        spec.env.insert("AURELIA_APP_ID".to_string(), app_id.to_string());
        spec.env.insert("AURELIA_APP_NAME".to_string(), app_name);
        if let Some(dir) = &game_dir {
            spec.env
                .insert("AURELIA_GAME_DIR".to_string(), dir.to_string_lossy().to_string());
        }
        spec.env.insert(
            "AURELIA_LAUNCH_PROGRAM".to_string(),
            old_program.to_string_lossy().to_string(),
        );
        spec.env
            .insert("AURELIA_LAUNCH_ARGS".to_string(), old_args.join(" "));

        // Rewrite the spec so the script wraps the resolved command as "$@".
        let mut new_args = Vec::with_capacity(old_args.len() + 1);
        new_args.push(old_program.to_string_lossy().to_string());
        new_args.extend(old_args);
        spec.args = new_args;
        spec.program = script.clone();
        // cwd is intentionally preserved.

        if let Some(logger) = &ctx.logger {
            let mut metadata = HashMap::new();
            metadata.insert("launch_script".to_string(), script.to_string_lossy().to_string());
            metadata.insert(
                "wrapped_program".to_string(),
                old_program.to_string_lossy().to_string(),
            );
            let _ = logger.info(
                "launch_script_applied",
                format!("Wrapping launch through script: {}", script.display()),
                Some("ApplyLaunchScript".to_string()),
                metadata,
            );
        }

        Ok(())
    }
}

#[cfg(test)]
#[path = "apply_launch_script_tests.rs"]
mod tests;
