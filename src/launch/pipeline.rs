use async_trait::async_trait;

use crate::launch::context::*;
use crate::launch::error::*;
use std::collections::HashMap;

#[async_trait]
pub trait PipelineStage: Send + Sync {
    fn name(&self) -> &str;
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError>;
}

pub struct LaunchPipeline {
    stages: Vec<Box<dyn PipelineStage>>,
    validators: Vec<Box<dyn crate::launch::validators::LaunchValidator>>,
}

impl Default for LaunchPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl LaunchPipeline {
    pub fn new() -> Self {
        Self {
            stages: Vec::new(),
            validators: Vec::new(),
        }
    }

    pub fn add_stage(&mut self, stage: Box<dyn PipelineStage>) {
        self.stages.push(stage);
    }

    pub fn add_validator(
        &mut self,
        validator: Box<dyn crate::launch::validators::LaunchValidator>,
    ) {
        self.validators.push(validator);
    }

    pub fn with_default_stages() -> Self {
        let mut pipeline = Self::new();
        pipeline.add_stage(Box::new(
            crate::launch::stages::resolve_game::ResolveGameStage,
        ));
        pipeline.add_stage(Box::new(
            crate::launch::stages::resolve_profile::ResolveProfileStage,
        ));
        pipeline.add_stage(Box::new(
            crate::launch::stages::resolve_game_fixups::ResolveGameFixupsStage,
        ));
        pipeline.add_stage(Box::new(
            crate::launch::stages::resolve_components::ResolveComponentsStage,
        ));
        pipeline.add_stage(Box::new(
            crate::launch::stages::resolve_dll_providers::ResolveDllProvidersStage,
        ));
        pipeline.add_stage(Box::new(
            crate::launch::stages::prepare_prefix::PreparePrefixStage,
        ));
        pipeline.add_stage(Box::new(
            crate::launch::stages::build_environment::BuildEnvironmentStage,
        ));
        pipeline.add_stage(Box::new(
            crate::launch::stages::build_command::BuildCommandStage,
        ));
        pipeline.add_stage(Box::new(
            crate::launch::stages::precache_shaders::PrecacheShadersStage,
        ));
        pipeline.add_stage(Box::new(crate::launch::stages::preflight::PreflightStage));
        pipeline.add_stage(Box::new(
            crate::launch::stages::user_launch_options::UserLaunchOptionsStage,
        ));
        pipeline.add_stage(Box::new(
            crate::launch::stages::apply_launch_script::ApplyLaunchScriptStage,
        ));
        pipeline.add_stage(Box::new(
            crate::launch::stages::spawn_process::SpawnProcessStage,
        ));
        pipeline.add_stage(Box::new(crate::launch::stages::finalize::FinalizeStage));

        pipeline.add_validator(Box::new(
            crate::launch::validators::overrides::OverrideConflictValidator,
        ));
        pipeline.add_validator(Box::new(
            crate::launch::validators::invariants::LaunchInvariantValidator,
        ));

        pipeline
    }

    pub async fn run(&self, ctx: &mut PipelineContext) -> std::result::Result<(), PipelineError> {
        // Record expected stack info before starting
        self.populate_expected_graphics_stack(ctx);

        // Initialize environment propagation tracking
        ctx.graphics_stack.env_propagation = HashMap::new();

        // After ResolveDllProvidersStage and BuildEnvironmentStage, we can populate effective state
        // but for now we'll do it right before validators if possible or within validators.
        // Actually, let's run validators twice: early and late.
        // Or just let stages populate the context.

        use crate::logging::LaunchResult;

        let total_start = std::time::Instant::now();
        let mut stage_durations = HashMap::new();
        let mut failing_stage = None;

        let mut metadata = HashMap::new();
        metadata.insert("app_id".to_string(), ctx.app_id.to_string());
        if let Some(app) = &ctx.app {
            metadata.insert("app_name".to_string(), app.name.clone());
        }
        ctx.log_info(
            "launch_start",
            "Starting launch pipeline".to_string(),
            None,
            metadata,
        );

        let mut final_result = LaunchResult::Success;

        // Run validators before starting stages
        for validator in &self.validators {
            validator.validate(ctx);
        }

        for stage in &self.stages {
            let stage_name = stage.name().to_string();
            ctx.log_info(
                "stage_start",
                format!("Starting stage: {}", stage_name),
                Some(stage_name.clone()),
                HashMap::new(),
            );

            let start_time = std::time::Instant::now();
            let res = stage.execute(ctx).await;
            let duration = start_time.elapsed().as_millis();
            stage_durations.insert(stage_name.clone(), duration);

            if let Err(e) = res {
                failing_stage = Some(stage_name.clone());
                final_result = LaunchResult::Failure;

                let mut metadata = HashMap::new();
                metadata.insert("error_kind".to_string(), e.kind.to_string());
                metadata.insert("error_message".to_string(), e.message.clone());
                metadata.insert("duration_ms".to_string(), duration.to_string());
                for (k, v) in &e.context {
                    metadata.insert(format!("error_ctx_{}", k), v.clone());
                }
                ctx.log_error(
                    "stage_failure",
                    format!("Stage failed: {}", stage_name),
                    Some(stage_name.clone()),
                    metadata.clone(),
                );

                ctx.log_error("launch_end", "Launch failed".to_string(), None, metadata);

                self.write_summary_if_possible(
                    ctx,
                    final_result,
                    failing_stage,
                    total_start.elapsed().as_millis(),
                    stage_durations.clone(),
                );

                return Err(PipelineError {
                    stage_name,
                    inner: e,
                });
            }

            let mut metadata = HashMap::new();
            metadata.insert("duration_ms".to_string(), duration.to_string());
            ctx.log_info(
                "stage_success",
                format!("Stage succeeded: {}", stage_name),
                Some(stage_name.clone()),
                metadata,
            );
        }

        ctx.log_info(
            "launch_end",
            "Process spawned successfully".to_string(),
            None,
            HashMap::new(),
        );

        // Post-spawn verification window
        self.verify_launch_health(ctx).await;

        let mut metadata = HashMap::new();
        metadata.insert("status".to_string(), ctx.verification.status.clone());
        if let Some(lifetime) = ctx.verification.process_lifetime_ms {
            metadata.insert("lifetime_ms".to_string(), lifetime.to_string());
        }
        if let Some(code) = ctx.verification.exit_code {
            metadata.insert("exit_code".to_string(), code.to_string());
        }
        ctx.log_info(
            "launch_verification",
            "Launch health verification complete".to_string(),
            None,
            metadata,
        );

        // After stages are complete (or failed), populate effective stack and scan logs for evidence
        self.record_dll_provider_diagnostics(ctx);

        // Sync architecture to diagnostics
        ctx.graphics_stack.target_architecture = ctx.target_architecture;

        // Populate effective graphics stack info from command spec/env BEFORE scanning logs
        // so that evidence expectations align with what was actually resolved.
        self.populate_effective_graphics_stack(ctx);

        // Track environment propagation
        if let Some(spec) = &ctx.command_spec {
            for key in &[
                "DXVK_HUD",
                "VKD3D_DEBUG",
                "DRI_PRIME",
                "__NV_PRIME_RENDER_OFFLOAD",
            ] {
                if spec.env.contains_key(*key) {
                    ctx.graphics_stack
                        .env_propagation
                        .insert(key.to_string(), true);
                }
            }
        }

        self.scan_logs_for_graphics_evidence(ctx).await;

        // Final verification adjustment based on evidence
        if ctx.verification.status == "verified" {
            let dxvk_requested = ctx.graphics_stack.runtime_evidence.dxvk.expected;
            let dxvk_found = ctx.graphics_stack.runtime_evidence.dxvk.evidence_found;

            if dxvk_requested && !dxvk_found && !ctx.verification.log_growth_observed {
                ctx.verification.status = "uncertain".to_string();
            }
        }

        // Run validators again on the final effective config
        for validator in &self.validators {
            validator.validate(ctx);
        }

        // Determine if policy was satisfied
        let mut policy_satisfied = true;
        if !ctx.graphics_stack.requested_backend.is_empty()
            && ctx.graphics_stack.requested_backend != "Auto"
        {
            let requested = ctx.graphics_stack.requested_backend.as_str();
            let effective = ctx.graphics_stack.effective_backend.as_str();
            if (requested == "DXVK" && effective != "DXVK")
                || (requested == "WineD3D" && effective != "WineD3D (Baseline)")
            {
                policy_satisfied = false;
            }
        }

        if !policy_satisfied && final_result == LaunchResult::Success {
            if ctx.graphics_stack.requested_backend == "DXVK" {
                final_result = LaunchResult::Failure;
            } else {
                final_result = LaunchResult::Degraded;
            }
        }

        // Adjust result based on verification
        if ctx.verification.status == "failed_after_spawn" {
            final_result = LaunchResult::Failure;
        } else if ctx.verification.status == "uncertain" && final_result == LaunchResult::Success {
            final_result = LaunchResult::Uncertain;
        }

        self.classify_early_exit(ctx);
        self.check_prefix_health(ctx);

        self.write_summary_if_possible(
            ctx,
            final_result,
            failing_stage,
            total_start.elapsed().as_millis(),
            stage_durations,
        );

        let msg = match final_result {
            LaunchResult::Success => "Launch successful".to_string(),
            LaunchResult::Failure => "Launch failed".to_string(),
            LaunchResult::Degraded => "Launch successful (degraded)".to_string(),
            LaunchResult::Uncertain => "Launch uncertain".to_string(),
        };
        ctx.log_info("launch_final_status", msg, None, HashMap::new());

        Ok(())
    }
}

#[cfg(test)]
mod tests;
