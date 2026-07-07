use std::fmt;
use async_trait::async_trait;

use std::collections::HashMap;
use crate::models::{LibraryGame, UserAppConfig};
use crate::config::LauncherConfig;
use crate::steam_client::LaunchInfo;
use crate::infra::runners::{Runner, CommandSpec};
use crate::infra::logging::{LaunchSession, EventLogger};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompatibilityWarning {
    pub code: String,
    pub message: String,
    pub context: HashMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct DuplicateInstanceInfo {
    pub detected: bool,
    pub source: String, // "lockfile" | "pid" | "guard" | "none"
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct GraphicsStackInfo {
    pub graphics_stack_expected: String,           // e.g. "DXVK v2.3, VKD3D-Proton v2.10"
    pub graphics_stack_evidence: Vec<String>,      // e.g. ["DXVK: v2.3.1"]
    pub graphics_stack_confidence: String,         // "low" | "medium" | "high"
    pub override_policy: String,                   // e.g. "Native-only"
    pub dll_providers: HashMap<String, String>,    // e.g. {"d3d11": "Runner", "d3d9": "GameLocal"}
    pub requested_backend: String,
    pub effective_backend: String,
    pub requested_d3d12_provider: String,
    pub effective_d3d12_provider: String,
    pub requested_nvapi: bool,
    pub effective_nvapi: bool,
    pub requested_gpu: Option<String>,
    pub effective_gpu: Option<String>,
    pub target_architecture: crate::models::ExecutableArchitecture,
    pub fallback_reasons: HashMap<String, String>,
    pub runtime_evidence: RuntimeEvidence,
    pub env_propagation: HashMap<String, bool>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct RuntimeEvidence {
    pub dxvk: EvidenceItem,
    pub vkd3d_proton: EvidenceItem,
    pub vkd3d: EvidenceItem,
    pub wined3d: EvidenceItem,
    pub scan_metadata: EvidenceScanMetadata,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct EvidenceScanMetadata {
    pub log_path: String,
    pub file_exists: bool,
    pub file_size: u64,
    pub line_count: usize,
    pub scan_duration_ms: u128,
    pub candidate_matches: usize,
    pub retries: usize,
    pub process_running_during_scan: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct EvidenceItem {
    pub expected: bool,
    pub evidence_found: bool,
    pub evidence: Vec<String>,
    pub diagnosis: String,
    pub diagnostics_note: String,
}

pub struct PipelineContext {
    pub app_id: u32,
    pub app: Option<LibraryGame>,
    pub launch_info: Option<LaunchInfo>,
    pub launcher_config: Option<LauncherConfig>,
    pub user_config: Option<UserAppConfig>,
    pub proton_path: Option<String>,
    /// One-off request (e.g. `aurelia play --native-engine`) to force the luxtorpeda
    /// runner for this launch regardless of the per-game config.
    pub force_native_engine: bool,
    /// One-off request (e.g. `aurelia play --umu`) to wrap this launch through the
    /// umu-launcher plugin regardless of the per-game config.
    pub force_umu: bool,
    /// Whether umu wrapping is active for this launch, resolved in
    /// `ResolveComponentsStage` and threaded into `LaunchContext`.
    pub use_umu: bool,
    /// The plugin-resolved absolute `umu-run` path (set when `use_umu`).
    pub umu_run: Option<std::path::PathBuf>,
    /// Run the game with real Steam integration
    pub steam_enabled: bool,

    pub resolved_install_dir: Option<std::path::PathBuf>,
    pub resolved_executable_path: Option<std::path::PathBuf>,
    pub executable_exists: bool,
    pub target_architecture: crate::models::ExecutableArchitecture,

    pub runner: Option<Box<dyn Runner>>,
    pub command_spec: Option<CommandSpec>,
    pub child: Option<std::process::Child>,

    pub session: Option<LaunchSession>,
    pub logger: Option<EventLogger>,
    pub warnings: Vec<CompatibilityWarning>,
    pub graphics_stack: GraphicsStackInfo,
    pub dll_resolutions: Vec<crate::launch::dll_provider_resolver::DllResolution>,
    /// Per-game auto-fixups resolved by `ResolveGameFixupsStage` and merged into the
    /// launch environment by the runner. Empty when the game has no registry entry.
    pub game_fixups: crate::launch::fixups::GameFixups,
    pub verification: crate::infra::logging::LaunchVerification,
}

impl PipelineContext {
    pub fn new(app_id: u32) -> Self {
        Self {
            app_id,
            app: None,
            launch_info: None,
            launcher_config: None,
            user_config: None,
            proton_path: None,
            force_native_engine: false,
            force_umu: false,
            use_umu: false,
            umu_run: None,
            steam_enabled: false,
            resolved_install_dir: None,
            resolved_executable_path: None,
            executable_exists: false,
            target_architecture: crate::models::ExecutableArchitecture::Unknown,
            runner: None,
            command_spec: None,
            child: None,
            session: None,
            logger: None,
            warnings: Vec::new(),
            graphics_stack: GraphicsStackInfo::default(),
            dll_resolutions: Vec::new(),
            game_fixups: crate::launch::fixups::GameFixups::default(),
            verification: crate::infra::logging::LaunchVerification::default(),
        }
    }

    pub fn add_warning(&mut self, code: impl Into<String>, message: impl Into<String>) {
        let warning = CompatibilityWarning {
            code: code.into(),
            message: message.into(),
            context: HashMap::new(),
        };

        if let Some(logger) = &self.logger {
            let mut metadata = HashMap::new();
            metadata.insert("warning_code".to_string(), warning.code.clone());
            metadata.insert("warning_message".to_string(), warning.message.clone());
            let _ = logger.log(crate::infra::logging::LogLevel::Warn, "compatibility_warning", warning.message.clone(), None, metadata);
        }

        self.warnings.push(warning);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LaunchErrorKind {
    Validation,      // Missing app context, invalid app id
    Environment,     // Missing proton path, config issues
    Permission,      // Failed to create dir, set permissions
    Runner,          // Runner prepare_prefix/build_command failed
    GameData,        // appmanifest not found, executable not found
    Process,         // Failed to spawn process
    Dependency,      // Missing MangoHud, etc.
    Unknown,
}

impl fmt::Display for LaunchErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Validation => "Validation",
            Self::Environment => "Environment",
            Self::Permission => "Permission",
            Self::Runner => "Runner",
            Self::GameData => "Game Data",
            Self::Process => "Process",
            Self::Dependency => "Dependency",
            Self::Unknown => "Unknown",
        };
        write!(f, "{}", s)
    }
}

impl LaunchErrorKind {
    pub fn remediation_hint(&self) -> &'static str {
        match self {
            Self::Validation => "Check if the game is correctly imported.",
            Self::Environment => "Verify your Global Settings and Compatibility Layer path.",
            Self::Permission => "Check filesystem permissions for the library and prefix folders.",
            Self::Runner => "Try a different Proton/Wine version.",
            Self::GameData => "Verify integrity of game files or reinstall the game.",
            Self::Process => "Check if the game is already running or if files are locked.",
            Self::Dependency => "Install missing system dependencies.",
            Self::Unknown => "Check the detailed logs for more information.",
        }
    }
}

#[derive(Debug)]
pub struct LaunchError {
    pub kind: LaunchErrorKind,
    pub message: String,
    pub context: HashMap<String, String>,
    pub source: Option<anyhow::Error>,
}

impl LaunchError {
    pub fn new(kind: LaunchErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            context: HashMap::new(),
            source: None,
        }
    }

    pub fn with_context(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }

    pub fn with_source(mut self, source: anyhow::Error) -> Self {
        self.source = Some(source);
        self
    }
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {} - {}", self.kind, self.message, self.kind.remediation_hint())
    }
}

impl std::error::Error for LaunchError {}

pub fn map_anyhow_error(err: anyhow::Error) -> LaunchError {
    let msg = err.to_string();
    if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
        return map_io_error(io_err, None);
    }

    if msg.contains("Permission denied") || msg.contains("EACCES") {
        LaunchError::new(LaunchErrorKind::Permission, msg).with_source(err)
    } else if msg.contains("not found") || msg.contains("No such file or directory") {
        LaunchError::new(LaunchErrorKind::GameData, msg).with_source(err)
    } else if msg.contains("proton") || msg.contains("compatibility layer") {
        LaunchError::new(LaunchErrorKind::Runner, msg).with_source(err)
    } else {
        LaunchError::new(LaunchErrorKind::Unknown, msg).with_source(err)
    }
}

pub fn map_io_error(err: &std::io::Error, dup_info: Option<&DuplicateInstanceInfo>) -> LaunchError {
    use std::io::ErrorKind;

    let kind = match err.kind() {
        ErrorKind::NotFound => LaunchErrorKind::GameData,
        ErrorKind::PermissionDenied => LaunchErrorKind::Permission,
        ErrorKind::InvalidInput => LaunchErrorKind::Validation,
        _ => LaunchErrorKind::Process,
    };

    let mut message = match err.kind() {
        ErrorKind::NotFound => "The game executable or runner was not found.".to_string(),
        ErrorKind::PermissionDenied => "Access denied while attempting to start the game. Check file permissions.".to_string(),
        ErrorKind::InvalidInput => "Invalid launch configuration or malformed path.".to_string(),
        ErrorKind::AlreadyExists => "A lock file or another instance of the game was detected.".to_string(),
        _ => format!("Process spawn failed: {}", err),
    };

    // ONLY show duplicate instance hint if explicitly detected or AlreadyExists kind
    let err_str = err.to_string();
    let lock_msg = "Game files are locked by another process. Ensure no other instance is running.";

    if let Some(info) = dup_info {
        if info.detected {
            message = lock_msg.to_string();
        }
    } else if err.kind() == ErrorKind::AlreadyExists || err_str.contains("locked") || err_str.contains("Resource busy") {
        message = lock_msg.to_string();
    }

    let mut launch_err = LaunchError::new(kind, message)
        .with_context("io_kind", format!("{:?}", err.kind()));

    if let Some(code) = err.raw_os_error() {
        launch_err = launch_err.with_context("os_errno", code.to_string());
    }

    let (detected, source) = match dup_info {
        Some(info) => (info.detected.to_string(), info.source.clone()),
        None => ("false".to_string(), "none".to_string()),
    };
    launch_err = launch_err.with_context("duplicate_instance_detected", detected);
    launch_err = launch_err.with_context("duplicate_detection_source", source);

    launch_err
}

/// Mark an evidence item as found and record the supporting line, capping the
/// stored evidence at 5 entries to bound memory/serialized size.
fn record_evidence(item: &mut EvidenceItem, line: String) {
    item.evidence_found = true;
    if item.evidence.len() < 5 {
        item.evidence.push(line);
    }
}

pub fn detect_duplicate_instance(ctx: &PipelineContext) -> DuplicateInstanceInfo {
    if let Some(spec) = &ctx.command_spec {
        // 1. Check for explicit lockfile in the game directory or prefix
        if let Some(cwd) = &spec.cwd {
            if cwd.join(".aurelia_launch.lock").exists() {
                return DuplicateInstanceInfo {
                    detected: true,
                    source: "lockfile".to_string(),
                };
            }
        }

        // 2. Check for tracked PID if we had a mechanism to store it
        // For now, check if steam.pid exists in the prefix (if applicable)
        if let Some(prefix) = spec.env.get("WINEPREFIX") {
            if std::path::Path::new(prefix).join("steam.pid").exists() {
                 // Note: Ideally we'd check if the PID is still alive,
                 // but existence of the file in WINEPREFIX is a strong hint.
                 return DuplicateInstanceInfo {
                     detected: true,
                     source: "pid".to_string(),
                 };
            }
        }
    }

    DuplicateInstanceInfo {
        detected: false,
        source: "none".to_string(),
    }
}

#[derive(Debug)]
pub struct PipelineError {
    pub stage_name: String,
    pub inner: LaunchError,
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pipeline failed at stage '{}': {}", self.stage_name, self.inner)
    }
}

impl std::error::Error for PipelineError {}

#[async_trait]
pub trait PipelineStage: Send + Sync {
    fn name(&self) -> &str;
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError>;
}

pub struct LaunchPipeline {
    stages: Vec<Box<dyn PipelineStage>>,
    validators: Vec<Box<dyn crate::launch::validators::LaunchValidator>>,
}

impl LaunchPipeline {
    async fn verify_launch_health(&self, ctx: &mut PipelineContext) {
        if let Some(child) = &mut ctx.child {
            let start_wait = std::time::Instant::now();
            let verify_duration = std::time::Duration::from_millis(2000);

            // Initial wait
            tokio::time::sleep(verify_duration).await;

            match child.try_wait() {
                Ok(Some(status)) => {
                    // Process exited already
                    ctx.verification.status = "failed_after_spawn".to_string();
                    ctx.verification.process_lifetime_ms = Some(start_wait.elapsed().as_millis() as u64);
                    ctx.verification.exit_code = status.code();
                }
                Ok(None) => {
                    // Process still running
                    ctx.verification.status = "verified".to_string();
                    ctx.verification.process_lifetime_ms = Some(start_wait.elapsed().as_millis() as u64);
                }
                Err(e) => {
                    ctx.verification.status = "uncertain".to_string();
                    if let Some(logger) = &ctx.logger {
                         let _ = logger.error("verification_error", format!("Failed to poll process status: {}", e), None, HashMap::new());
                    }
                }
            }
        } else {
            ctx.verification.status = "not_verified".to_string();
        }
    }

    fn classify_early_exit(&self, ctx: &mut PipelineContext) {
        if ctx.verification.status != "failed_after_spawn" {
            if ctx.verification.steam_runtime_milestone == "steam_process_exited_early" || ctx.verification.steam_auto_start_failed {
                ctx.verification.detailed_status = Some("steam_runtime_startup_failed".to_string());
                return;
            }

            if ctx.command_spec.is_none() {
                ctx.verification.detailed_status = Some("command_spec_missing".to_string());
                return;
            }

            if !ctx.executable_exists && ctx.resolved_executable_path.is_some() {
                ctx.verification.detailed_status = Some("game_executable_not_found".to_string());
                return;
            }

            return;
        }

        let dxvk_found = ctx.graphics_stack.runtime_evidence.dxvk.evidence_found;
        let vkd3d_found = ctx.graphics_stack.runtime_evidence.vkd3d_proton.evidence_found || ctx.graphics_stack.runtime_evidence.vkd3d.evidence_found;

        let mut fatal_error = None;
        for evidence in &ctx.graphics_stack.graphics_stack_evidence {
            if evidence.contains("Override Policy Conflict") {
                 fatal_error = Some("override_policy_regression");
                 break;
            }
            if evidence.contains("Steam Client/Environment Failure") {
                 fatal_error = Some("steam_runtime_exposure_regression");
                 break;
            }
            if evidence.contains("PhysX/Middleware failure") {
                fatal_error = Some("middleware_dependency_failure");
                break;
            }
            if evidence.contains("SteamAPI Initialization Failed") || evidence.contains("SteamAPI Access Violation") {
                fatal_error = Some("steamapi_init_failed");
                break;
            }
            if evidence.contains("Steam Ownership Validation Failed") {
                fatal_error = Some("steam_ownership_or_session_failed");
                break;
            }
            if evidence.contains("SteamAPI Connection Failed") {
                fatal_error = Some("steam_not_found_by_game");
                break;
            }
            if evidence.contains("DLL Load Failure") {
                if evidence.contains("d3d11") {
                     fatal_error = Some("startup_environment_regression");
                } else {
                     fatal_error = Some("missing_required_module");
                }
                break;
            }
            if evidence.contains("DLL Dependency Missing") {
                fatal_error = Some("middleware_or_runtime_component_failure");
                break;
            }
        }

        ctx.verification.detailed_status = Some(if dxvk_found || vkd3d_found {
            "game_failed_after_spawn".to_string()
        } else if let Some(err) = fatal_error {
            err.to_string()
        } else if ctx.verification.steam_running_before_launch && ctx.verification.steam_client_exposed {
            "steam_sensitive_game_failed_after_spawn".to_string()
        } else {
            "game_failed_after_spawn".to_string()
        });
    }

    pub fn new() -> Self {
        Self {
            stages: Vec::new(),
            validators: Vec::new(),
        }
    }

    pub fn add_stage(&mut self, stage: Box<dyn PipelineStage>) {
        self.stages.push(stage);
    }

    pub fn add_validator(&mut self, validator: Box<dyn crate::launch::validators::LaunchValidator>) {
        self.validators.push(validator);
    }

    pub fn with_default_stages() -> Self {
        let mut pipeline = Self::new();
        pipeline.add_stage(Box::new(crate::launch::stages::resolve_game::ResolveGameStage));
        pipeline.add_stage(Box::new(crate::launch::stages::resolve_profile::ResolveProfileStage));
        pipeline.add_stage(Box::new(crate::launch::stages::resolve_game_fixups::ResolveGameFixupsStage));
        pipeline.add_stage(Box::new(crate::launch::stages::resolve_components::ResolveComponentsStage));
        pipeline.add_stage(Box::new(crate::launch::stages::resolve_dll_providers::ResolveDllProvidersStage));
        pipeline.add_stage(Box::new(crate::launch::stages::prepare_prefix::PreparePrefixStage));
        pipeline.add_stage(Box::new(crate::launch::stages::build_environment::BuildEnvironmentStage));
        pipeline.add_stage(Box::new(crate::launch::stages::build_command::BuildCommandStage));
        pipeline.add_stage(Box::new(crate::launch::stages::preflight::PreflightStage));
        pipeline.add_stage(Box::new(crate::launch::stages::spawn_process::SpawnProcessStage));
        pipeline.add_stage(Box::new(crate::launch::stages::finalize::FinalizeStage));

        pipeline.add_validator(Box::new(crate::launch::validators::overrides::OverrideConflictValidator));
        pipeline.add_validator(Box::new(crate::launch::validators::invariants::LaunchInvariantValidator));

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

        use crate::infra::logging::LaunchResult;

        let total_start = std::time::Instant::now();
        let mut stage_durations = HashMap::new();
        let mut failing_stage = None;

        if let Some(logger) = &ctx.logger {
            let mut metadata = HashMap::new();
            metadata.insert("app_id".to_string(), ctx.app_id.to_string());
            if let Some(app) = &ctx.app {
                metadata.insert("app_name".to_string(), app.name.clone());
            }
            let _ = logger.info("launch_start", "Starting launch pipeline".to_string(), None, metadata);
        }

        let mut final_result = LaunchResult::Success;

        // Run validators before starting stages
        for validator in &self.validators {
            validator.validate(ctx);
        }

        for stage in &self.stages {
            let stage_name = stage.name().to_string();
            if let Some(logger) = &ctx.logger {
                let _ = logger.info("stage_start", format!("Starting stage: {}", stage_name), Some(stage_name.clone()), HashMap::new());
            }

            let start_time = std::time::Instant::now();
            let res = stage.execute(ctx).await;
            let duration = start_time.elapsed().as_millis();
            stage_durations.insert(stage_name.clone(), duration);

            if let Err(e) = res {
                failing_stage = Some(stage_name.clone());
                final_result = LaunchResult::Failure;

                if let Some(logger) = &ctx.logger {
                    let mut metadata = HashMap::new();
                    metadata.insert("error_kind".to_string(), e.kind.to_string());
                    metadata.insert("error_message".to_string(), e.message.clone());
                    metadata.insert("duration_ms".to_string(), duration.to_string());
                    for (k, v) in &e.context {
                        metadata.insert(format!("error_ctx_{}", k), v.clone());
                    }
                    let _ = logger.error("stage_failure", format!("Stage failed: {}", stage_name), Some(stage_name.clone()), metadata.clone());

                    let _ = logger.error("launch_end", "Launch failed".to_string(), None, metadata);
                }

                self.write_summary_if_possible(ctx, final_result, failing_stage, total_start.elapsed().as_millis(), stage_durations.clone());

                return Err(PipelineError {
                    stage_name,
                    inner: e,
                });
            }

            if let Some(logger) = &ctx.logger {
                let mut metadata = HashMap::new();
                metadata.insert("duration_ms".to_string(), duration.to_string());
                let _ = logger.info("stage_success", format!("Stage succeeded: {}", stage_name), Some(stage_name.clone()), metadata);
            }
        }

        if let Some(logger) = &ctx.logger {
            let _ = logger.info("launch_end", "Process spawned successfully".to_string(), None, HashMap::new());
        }

        // Post-spawn verification window
        self.verify_launch_health(ctx).await;

        if let Some(logger) = &ctx.logger {
            let mut metadata = HashMap::new();
            metadata.insert("status".to_string(), ctx.verification.status.clone());
            if let Some(lifetime) = ctx.verification.process_lifetime_ms {
                metadata.insert("lifetime_ms".to_string(), lifetime.to_string());
            }
            if let Some(code) = ctx.verification.exit_code {
                metadata.insert("exit_code".to_string(), code.to_string());
            }
            let _ = logger.info("launch_verification", "Launch health verification complete".to_string(), None, metadata);
        }

        // After stages are complete (or failed), populate effective stack and scan logs for evidence
        self.record_dll_provider_diagnostics(ctx);

        // Sync architecture to diagnostics
        ctx.graphics_stack.target_architecture = ctx.target_architecture;

        // Populate effective graphics stack info from command spec/env BEFORE scanning logs
        // so that evidence expectations align with what was actually resolved.
        self.populate_effective_graphics_stack(ctx);

        // Track environment propagation
        if let Some(spec) = &ctx.command_spec {
             for key in &["DXVK_HUD", "VKD3D_DEBUG", "DRI_PRIME", "__NV_PRIME_RENDER_OFFLOAD"] {
                 if spec.env.contains_key(*key) {
                     ctx.graphics_stack.env_propagation.insert(key.to_string(), true);
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
        if !ctx.graphics_stack.requested_backend.is_empty() && ctx.graphics_stack.requested_backend != "Auto" {
            if ctx.graphics_stack.requested_backend == "DXVK" && ctx.graphics_stack.effective_backend != "DXVK" {
                policy_satisfied = false;
            } else if ctx.graphics_stack.requested_backend == "WineD3D" && ctx.graphics_stack.effective_backend != "WineD3D (Baseline)" {
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

        self.write_summary_if_possible(ctx, final_result, failing_stage, total_start.elapsed().as_millis(), stage_durations);

        if let Some(logger) = &ctx.logger {
            let msg = match final_result {
                LaunchResult::Success => "Launch successful".to_string(),
                LaunchResult::Failure => "Launch failed".to_string(),
                LaunchResult::Degraded => "Launch successful (degraded)".to_string(),
                LaunchResult::Uncertain => "Launch uncertain".to_string(),
            };
            let _ = logger.info("launch_final_status", msg, None, HashMap::new());
        }

        Ok(())
    }

    fn populate_effective_graphics_stack(&self, ctx: &mut PipelineContext) {
        if let Some(spec) = &ctx.command_spec {
             if let Some(overrides) = spec.env.get("WINEDLLOVERRIDES") {
                 let has_dxvk = overrides.contains("d3d11=n") || overrides.contains("dxgi=n") || overrides.contains("d3d9=n") || overrides.contains("d3d8=n");
                 let has_vkd3dp = overrides.contains("d3d12=n");
                 let has_vkd3dw = overrides.contains("libvkd3d-1=n");

                 ctx.graphics_stack.effective_backend = if has_dxvk { "DXVK" } else { "WineD3D (Baseline)" }.to_string();
                 ctx.graphics_stack.effective_d3d12_provider = if has_vkd3dp { "vkd3d-proton" } else if has_vkd3dw { "vkd3d" } else { "None" }.to_string();

                 if has_dxvk || has_vkd3dp || has_vkd3dw {
                     ctx.graphics_stack.override_policy = "Native-preferred".to_string();
                 } else {
                     ctx.graphics_stack.override_policy = "Builtin-only".to_string();
                 }
             } else {
                 ctx.graphics_stack.effective_backend = "WineD3D (Baseline)".to_string();
                 ctx.graphics_stack.effective_d3d12_provider = "None".to_string();
                 ctx.graphics_stack.override_policy = "Builtin-only".to_string();
             }

             // Synchronize expected stack string with effective resolution
             let mut stack_parts = Vec::new();
             if ctx.graphics_stack.effective_backend == "DXVK" {
                 stack_parts.push("DXVK");
             }
             if ctx.graphics_stack.effective_d3d12_provider != "None" {
                 stack_parts.push(&ctx.graphics_stack.effective_d3d12_provider);
             }

             if stack_parts.is_empty() {
                 ctx.graphics_stack.graphics_stack_expected = "WineD3D (Baseline)".to_string();
             } else {
                 ctx.graphics_stack.graphics_stack_expected = stack_parts.join(", ");
             }

             let nvapi_res = ctx.dll_resolutions.iter().find(|r| r.name == "nvapi");
             ctx.graphics_stack.effective_nvapi = nvapi_res.map(|r| r.chosen_provider != crate::launch::dll_provider_resolver::DllProvider::None).unwrap_or(false);

             // GPU Selection
             ctx.graphics_stack.effective_gpu = None;
             if let Some(val) = spec.env.get("__NV_PRIME_RENDER_OFFLOAD") {
                 if val == "1" {
                     ctx.graphics_stack.effective_gpu = Some("NVIDIA Discrete GPU".to_string());
                 }
             } else if let Some(val) = spec.env.get("DRI_PRIME") {
                 if val == "1" {
                     ctx.graphics_stack.effective_gpu = Some("Secondary GPU (DRI_PRIME=1)".to_string());
                 } else if val == "0" {
                     ctx.graphics_stack.effective_gpu = Some("Primary GPU (DRI_PRIME=0)".to_string());
                 }
             }
        }
    }

    fn populate_expected_graphics_stack(&self, ctx: &mut PipelineContext) {
        if let Some(config) = &ctx.user_config {
            ctx.graphics_stack.requested_backend = format!("{:?}", config.graphics_layers.graphics_backend_policy);
            ctx.graphics_stack.requested_d3d12_provider = format!("{:?}", config.graphics_layers.d3d12_policy);
            ctx.graphics_stack.requested_nvapi = config.graphics_layers.nvapi_enabled;
            ctx.graphics_stack.requested_gpu = config.gpu_preference.clone();

            // Initial baseline assumption - will be overridden by populate_effective_graphics_stack
            ctx.graphics_stack.graphics_stack_expected = "WineD3D (Baseline)".to_string();
            ctx.graphics_stack.override_policy = "Builtin-only".to_string();
        }
    }

    fn record_dll_provider_diagnostics(&self, ctx: &mut PipelineContext) {
        for res in &ctx.dll_resolutions {
            ctx.graphics_stack.dll_providers.insert(res.name.clone(), format!("{:?}", res.chosen_provider));
            if let Some(reason) = &res.fallback_reason {
                ctx.graphics_stack.fallback_reasons.insert(res.name.clone(), reason.clone());
            }
        }
    }

    async fn scan_logs_for_graphics_evidence(&self, ctx: &mut PipelineContext) {
        let scan_start = std::time::Instant::now();
        if let Some(session) = &ctx.session {
            let log_path = ctx.command_spec.as_ref()
                .and_then(|spec| spec.env.get("WINE_LOG_OUTPUT"))
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| session.stderr_path());
            ctx.graphics_stack.runtime_evidence.scan_metadata.log_path = log_path.to_string_lossy().to_string();

            let mut retries = 0;
            let max_retries = 3;
            let mut content = String::new();

            let initial_file_size = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);

            while retries <= max_retries {
                if log_path.exists() {
                    ctx.graphics_stack.runtime_evidence.scan_metadata.file_exists = true;
                    let current_size = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
                    ctx.graphics_stack.runtime_evidence.scan_metadata.file_size = current_size;

                    if current_size > initial_file_size {
                        ctx.verification.log_growth_observed = true;
                    }

                    if let Ok(current_content) = std::fs::read_to_string(&log_path) {
                        if !current_content.is_empty() || retries == max_retries {
                            content = current_content;
                            break;
                        }
                    }
                }

                let process_running = if let Some(child) = &mut ctx.child {
                    child.try_wait().map(|s| s.is_none()).unwrap_or(false)
                } else {
                    false
                };
                ctx.graphics_stack.runtime_evidence.scan_metadata.process_running_during_scan = process_running;

                if !process_running && retries > 0 {
                    break;
                }

                retries += 1;
                ctx.graphics_stack.runtime_evidence.scan_metadata.retries = retries;
                tokio::time::sleep(std::time::Duration::from_millis(200 * retries as u64)).await;
            }

            if !content.is_empty() {
                let lines: Vec<&str> = content.lines().collect();
                ctx.graphics_stack.runtime_evidence.scan_metadata.line_count = lines.len();

                // Capture Log Head/Tail
                ctx.verification.log_head = lines.iter().take(50).map(|s| s.to_string()).collect();
                if lines.len() > 50 {
                     ctx.verification.log_tail = lines.iter().rev().take(100).rev().map(|s| s.to_string()).collect();
                }

                let mut max_milestone = crate::infra::logging::StartupMilestone::None;

                // Derive component paths from dll resolutions
                let mut component_paths = HashMap::new();
                for res in &ctx.dll_resolutions {
                    if res.chosen_provider == crate::launch::dll_provider_resolver::DllProvider::Runner {
                        if let Some(path) = &res.chosen_path {
                            if let Some(parent) = path.parent() {
                                let family = if res.name.contains("d3d12") || res.name.contains("vkd3d") {
                                    if path.to_string_lossy().contains("vkd3d-proton") { "vkd3d_proton" } else { "vkd3d" }
                                } else {
                                    "dxvk"
                                };
                                component_paths.insert(family, parent.to_string_lossy().to_string());
                            }
                        }
                    }
                }

                for line in &lines {
                    let mut line_matched = false;
                    let line_lower = line.to_lowercase();

                    // Detect Dependency Families from log lines
                    let families_to_scan = [
                        ("Batman (GFSDK/APEX)", vec!["gfsdk", "apex", "nvtt"]),
                        ("Metro (4A Engine)", vec!["4a engine", "metro"]),
                        ("PhysX Runtime", vec!["physx"]),
                    ];

                    for (family, patterns) in families_to_scan {
                        if patterns.iter().any(|p| line_lower.contains(p))
                            && !ctx.verification.dependency_families_detected.contains(&family.to_string())
                        {
                            ctx.verification.dependency_families_detected.push(family.to_string());
                        }
                    }

                    // Update Milestone
                    if let Some(m) = crate::infra::logging::detect_startup_milestone(line) {
                         if m > max_milestone {
                             max_milestone = m;
                         }
                    }

                    if let Some(evidence) = crate::infra::logging::classify_graphics_evidence(line) {
                        if !ctx.graphics_stack.graphics_stack_evidence.contains(&evidence) {
                            ctx.graphics_stack.graphics_stack_evidence.push(evidence.clone());
                        }

                        if evidence.contains("SteamAPI Initialization Failed") {
                            ctx.verification.steam_api_initialized = Some(false);
                        }
                        if evidence.contains("Steam Ownership Validation Failed") {
                            ctx.verification.steam_ownership_confirmed = Some(false);
                        }
                        if evidence.contains("Steam Client Artifact:") {
                            if evidence.contains("local") { ctx.verification.steam_client_artifact = Some("local".into()); }
                            else if evidence.contains("windows") { ctx.verification.steam_client_artifact = Some("windows".into()); }
                            else if evidence.contains("host") { ctx.verification.steam_client_artifact = Some("host".into()); }
                        }

                        if evidence.contains("DXVK") {
                            record_evidence(&mut ctx.graphics_stack.runtime_evidence.dxvk, evidence.clone());
                        }
                        if evidence.contains("VKD3D-Proton") {
                            record_evidence(&mut ctx.graphics_stack.runtime_evidence.vkd3d_proton, evidence.clone());
                        }
                        if evidence.contains("VKD3D") && !evidence.contains("Proton") {
                            record_evidence(&mut ctx.graphics_stack.runtime_evidence.vkd3d, evidence.clone());
                        }
                        if evidence.contains("WineD3D") {
                            record_evidence(&mut ctx.graphics_stack.runtime_evidence.wined3d, evidence.clone());
                        }
                        line_matched = true;
                    }

                    // Robust Path & Module Matching

                    // DXVK Path Match
                    if let Some(path) = component_paths.get("dxvk") {
                        if line_lower.contains(&path.to_lowercase()) {
                            record_evidence(&mut ctx.graphics_stack.runtime_evidence.dxvk, format!("Path match: {}", line.trim()));
                            line_matched = true;
                        }
                    }

                    // VKD3D-Proton Path Match
                    if let Some(path) = component_paths.get("vkd3d_proton") {
                        if line_lower.contains(&path.to_lowercase()) {
                            record_evidence(&mut ctx.graphics_stack.runtime_evidence.vkd3d_proton, format!("Path match: {}", line.trim()));
                            line_matched = true;
                        }
                    }

                    // Generic Module Loader matches for graphics DLLs
                    if line_lower.contains("loaddll") || line_lower.contains("load_module") || line_lower.contains("import_dll") {
                        let graphics_dlls = ["d3d9", "d3d11", "dxgi", "d3d12", "d3d8", "d3d10core", "libvkd3d"];
                        for dll in graphics_dlls {
                            if line_lower.contains(dll) {
                                let is_failed = line_lower.contains("failed") || line_lower.contains("error");
                                let is_builtin = line_lower.contains("system32") || (line_lower.contains("builtin") && !line_lower.contains("native"));

                                let evidence_type = if is_failed {
                                    "Module load FAILED"
                                } else if is_builtin {
                                    "Module load (builtin/fallback)"
                                } else {
                                    "Module load (likely native)"
                                };

                                let msg = format!("{}: {}", evidence_type, line.trim());
                                if !ctx.graphics_stack.graphics_stack_evidence.contains(&msg) {
                                    ctx.graphics_stack.graphics_stack_evidence.push(msg.clone());

                                    if is_builtin {
                                        record_evidence(&mut ctx.graphics_stack.runtime_evidence.wined3d, msg);
                                    }
                                }
                                line_matched = true;
                            }
                        }
                    }

                    if line_matched {
                        ctx.graphics_stack.runtime_evidence.scan_metadata.candidate_matches += 1;
                    }
                }

                ctx.verification.last_successful_startup_milestone = max_milestone.to_string();
            }
        } else {
            ctx.graphics_stack.runtime_evidence.scan_metadata.file_exists = false;
        }

        // Populate expectations
            ctx.graphics_stack.runtime_evidence.dxvk.expected = ctx.graphics_stack.effective_backend == "DXVK";
            ctx.graphics_stack.runtime_evidence.vkd3d_proton.expected = ctx.graphics_stack.effective_d3d12_provider == "vkd3d-proton";
            ctx.graphics_stack.runtime_evidence.vkd3d.expected = ctx.graphics_stack.effective_d3d12_provider == "vkd3d";
            ctx.graphics_stack.runtime_evidence.wined3d.expected = ctx.graphics_stack.effective_backend == "WineD3D (Baseline)";

            // Build diagnosis
            let diagnose = |item: &mut EvidenceItem, name: &str, metadata: &EvidenceScanMetadata| {
                if !metadata.file_exists {
                    item.diagnosis = format!("{} requested; Wine log missing", name);
                    item.diagnostics_note = format!("Log file was not found at {}.", metadata.log_path);
                    return;
                }
                if metadata.line_count == 0 {
                    if metadata.process_running_during_scan {
                        item.diagnosis = format!("{} requested; Wine log empty (early scan)", name);
                        item.diagnostics_note = format!("Log file exists but is empty. Process is still running (retries: {}). Evidence may appear later.", metadata.retries);
                    } else {
                        item.diagnosis = format!("{} requested; Wine log empty", name);
                        item.diagnostics_note = format!("Log file exists but contains no lines after {} retries. Process has exited. Logging may be disabled or failed to initialize.", metadata.retries);
                    }
                    return;
                }

                if item.expected {
                    if item.evidence_found {
                        item.diagnosis = format!("confirmed: {} loaded based on runtime evidence", name);
                    } else {
                        item.diagnosis = format!("inconclusive: {} requested but no runtime evidence found", name);
                        item.diagnostics_note = format!("Scanned {} lines, found {} matches for graphics patterns, but none confirmed {} usage.",
                            metadata.line_count, metadata.candidate_matches, name);
                    }
                } else if item.evidence_found {
                    item.diagnosis = format!("suspected fallback: {} not requested but runtime evidence found", name);
                } else {
                    item.diagnosis = "neutral: inconclusive".to_string();
                }
            };

            let meta = ctx.graphics_stack.runtime_evidence.scan_metadata.clone();
            diagnose(&mut ctx.graphics_stack.runtime_evidence.dxvk, "DXVK", &meta);
            diagnose(&mut ctx.graphics_stack.runtime_evidence.vkd3d_proton, "VKD3D-Proton", &meta);
            diagnose(&mut ctx.graphics_stack.runtime_evidence.vkd3d, "VKD3D", &meta);
            diagnose(&mut ctx.graphics_stack.runtime_evidence.wined3d, "WineD3D", &meta);

            // Refinement: Do not mark WineD3D fallback when primary components are confirmed
            if (ctx.graphics_stack.runtime_evidence.dxvk.evidence_found ||
                ctx.graphics_stack.runtime_evidence.vkd3d_proton.evidence_found ||
                ctx.graphics_stack.runtime_evidence.vkd3d.evidence_found) &&
               !ctx.graphics_stack.runtime_evidence.wined3d.expected
            {
                if ctx.graphics_stack.runtime_evidence.wined3d.evidence_found {
                    let has_strong_wined3d = ctx.graphics_stack.runtime_evidence.wined3d.evidence.iter()
                        .any(|e| e.contains("Fallback Detected"));
                    if !has_strong_wined3d {
                        ctx.graphics_stack.runtime_evidence.wined3d.diagnosis = "neutral: inconclusive (standard builtins loaded)".to_string();
                    }
                }
            }

            // Update confidence based on evidence vs expectation
            if ctx.graphics_stack.graphics_stack_evidence.is_empty() {
                ctx.graphics_stack.graphics_stack_confidence = "low".to_string();
            } else {
                let has_expected = !ctx.graphics_stack.runtime_evidence.dxvk.expected
                    || ctx.graphics_stack.runtime_evidence.dxvk.evidence_found;

                if has_expected {
                    ctx.graphics_stack.graphics_stack_confidence = "high".to_string();
                } else {
                    ctx.graphics_stack.graphics_stack_confidence = "medium".to_string();
                }
            }
        ctx.graphics_stack.runtime_evidence.scan_metadata.scan_duration_ms = scan_start.elapsed().as_millis();
    }

    fn check_prefix_health(&self, ctx: &mut PipelineContext) {
        if let Some(spec) = &ctx.command_spec {
            if let Some(prefix) = spec.env.get("WINEPREFIX") {
                let prefix_path = std::path::PathBuf::from(prefix);
                if !prefix_path.exists() {
                    ctx.add_warning("PREFIX_MISSING", format!("WINEPREFIX does not exist on disk: {}", prefix));
                    return;
                }

                let mut metadata = HashMap::new();
                metadata.insert("prefix_path".to_string(), prefix.clone());

                // Detect Windows username
                let users_dir = prefix_path.join("drive_c/users");
                if users_dir.exists() {
                    if let Ok(entries) = std::fs::read_dir(&users_dir) {
                        let mut usernames = Vec::new();
                        for entry in entries.flatten() {
                            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                                let name = entry.file_name().to_string_lossy().to_string();
                                if name != "Public" && name != "All Users" && name != "Default User" {
                                    usernames.push(name);
                                }
                            }
                        }
                        // Pick the alphabetically first (most likely primary)
                        if let Some(primary) = usernames.into_iter().min() {
                            ctx.verification.windows_user_path = Some(format!("C:\\users\\{}", primary));
                            metadata.insert("detected_windows_username".to_string(), primary.clone());
                            ctx.verification.windows_username = Some(primary);
                        }
                    }
                }

                let username = ctx.verification.windows_username.as_deref().unwrap_or("steamuser");
                let common_dirs = [
                    format!("drive_c/users/{}/Documents", username),
                    format!("drive_c/users/{}/AppData/Local", username),
                    format!("drive_c/users/{}/AppData/Roaming", username),
                    "drive_c/Program Files (x86)/Steam/steam.exe".to_string(),
                    "drive_c/Program Files (x86)/Steam/steamclient.dll".to_string(),
                    "drive_c/Program Files (x86)/Steam/tier0_s.dll".to_string(),
                    "drive_c/windows/system32/PhysXLoader.dll".to_string(),
                    "drive_c/windows/syswow64/PhysXLoader.dll".to_string(),
                    "drive_c/windows/system32/steam_api.dll".to_string(),
                    "drive_c/windows/syswow64/steam_api.dll".to_string(),
                    "drive_c/windows/system32/steam_api64.dll".to_string(),
                ];

                for dir in common_dirs {
                    let full_path = prefix_path.join(&dir);
                    let exists = full_path.exists();
                    metadata.insert(format!("path_exists:{}", dir), exists.to_string());
                    ctx.verification.key_paths_detected.insert(dir, exists);
                }

                // Title-Specific Dependency Detection
                let families = &mut ctx.verification.dependency_families_detected;
                let families_to_check = [
                    ("Batman (PhysX/APEX)", vec!["drive_c/windows/system32/PhysXLoader.dll", "drive_c/windows/syswow64/PhysXLoader.dll"]),
                    ("Amnesia (SDL2/Newton)", vec!["drive_c/windows/system32/SDL2.dll", "drive_c/windows/syswow64/SDL2.dll"]),
                    ("Metro (4A Engine)", vec!["drive_c/windows/system32/PhysXDevice.dll", "drive_c/windows/syswow64/PhysXDevice.dll"]),
                ];

                for (family, paths) in families_to_check {
                    if paths.iter().any(|p| prefix_path.join(p).exists())
                        && !families.contains(&family.to_string())
                    {
                        families.push(family.to_string());
                    }
                }

                // Check steam client exposure
                ctx.verification.steam_client_exposed = spec.env.contains_key("STEAM_COMPAT_CLIENT_INSTALL_PATH") ||
                                                       spec.env.get("WINEPATH").map(|wp| wp.contains("Steam")).unwrap_or(false);

                if let Some(logger) = &ctx.logger {
                    let _ = logger.info("prefix_health_check", "WINEPREFIX sanity check complete".to_string(), None, metadata);
                }
            }
        }
    }

    fn write_summary_if_possible(
        &self,
        ctx: &mut PipelineContext,
        result: crate::infra::logging::LaunchResult,
        failing_stage: Option<String>,
        total_duration_ms: u128,
        stage_durations_ms: HashMap<String, u128>,
    ) {
        let (sanity_warnings, env_snapshot) = if let Some(spec) = &ctx.command_spec {
            let runner_name = ctx
                .runner
                .as_ref()
                .map(|r| r.name().to_string())
                .unwrap_or_else(|| "Unknown".to_string());

            let warnings = crate::infra::logging::check_environment_sanity(
                &spec.env,
                &runner_name,
                ctx.user_config.as_ref(),
            );

            let env_snapshot = crate::infra::logging::EffectiveEnv {
                runner_name,
                profile_id: ctx.launch_info.as_ref().map(|l| l.id.clone()),
                profile_name: ctx.launch_info.as_ref().map(|l| l.description.clone()),
                wine_dll_overrides: spec.env.get("WINEDLLOVERRIDES").cloned(),
                env_vars: spec.env.clone(),
            };

            (warnings, Some(env_snapshot))
        } else {
            (Vec::new(), None)
        };

        for warning in sanity_warnings {
            ctx.add_warning(warning.code, warning.message);
        }

        if let Some(session) = &ctx.session {
            if let Some(env) = env_snapshot {
                let _ = session.write_effective_env(&env);
                let _ = session.write_effective_env_txt(&env.env_vars);
            }
            if let Some(spec) = &ctx.command_spec {
                let _ = session.write_command_artifact(spec);
            }
            let _ = session.write_dll_resolution_artifact(&ctx.dll_resolutions);

            // Write Effective Launch Config
            if let Some(spec) = &ctx.command_spec {
                 use crate::infra::logging::{EffectiveLaunchConfig, EffectiveGameConfig, EffectiveRunnerConfig, EffectiveSettingsConfig, EffectiveCommandConfig};
                 let config = EffectiveLaunchConfig {
                     session_id: session.id.to_string(),
                     app_id: ctx.app_id,
                     app_name: ctx.app.as_ref().map(|a| a.name.clone()),
                     game: EffectiveGameConfig {
                         install_dir: ctx.resolved_install_dir.clone(),
                         executable_path: ctx.resolved_executable_path.clone(),
                         executable_exists: ctx.executable_exists,
                     },
                     runner: EffectiveRunnerConfig {
                         name: ctx.runner.as_ref().map(|r| r.name().to_string()),
                         root: {
                             let proton = if let Some(forced) = ctx.launcher_config.as_ref()
                                 .and_then(|c| c.game_configs.get(&ctx.app_id))
                                 .and_then(|c| c.forced_proton_version.as_ref())
                             {
                                 forced.as_str()
                             } else {
                                 ctx.proton_path.as_deref().unwrap_or("wine")
                             };
                             let library_root = ctx.launcher_config.as_ref().map(|c| std::path::PathBuf::from(&c.steam_library_path)).unwrap_or_default();
                             let active_runner = crate::utils::resolve_runner(proton, &library_root);
                             Some(crate::utils::derive_runner_root(&active_runner))
                         },
                     },
                     settings: EffectiveSettingsConfig {
                         requested_backend: ctx.graphics_stack.requested_backend.clone(),
                         effective_backend: ctx.graphics_stack.effective_backend.clone(),
                         requested_d3d12_provider: ctx.graphics_stack.requested_d3d12_provider.clone(),
                         effective_d3d12_provider: ctx.graphics_stack.effective_d3d12_provider.clone(),
                         requested_nvapi: ctx.graphics_stack.requested_nvapi,
                         effective_nvapi: ctx.graphics_stack.effective_nvapi,
                         requested_gpu: ctx.graphics_stack.requested_gpu.clone(),
                         effective_gpu: ctx.graphics_stack.effective_gpu.clone(),
                         target_architecture: ctx.target_architecture,
                         dll_resolutions: ctx.dll_resolutions.clone(),
                         wine_dll_overrides: spec.env.get("WINEDLLOVERRIDES").cloned(),
                         runtime_evidence: Some(ctx.graphics_stack.runtime_evidence.clone()),
                         env_propagation: Some(ctx.graphics_stack.env_propagation.clone()),
                     },
                     command: EffectiveCommandConfig {
                         program: spec.program.clone(),
                         args: spec.args.clone(),
                         cwd: spec.cwd.clone(),
                         env_subset: {
                             let mut subset = HashMap::new();
                             for key in &["DRI_PRIME", "__NV_PRIME_RENDER_OFFLOAD", "__NV_PRIME_RENDER_OFFLOAD_PROVIDER", "DXVK_HUD", "VKD3D_DEBUG"] {
                                 if let Some(val) = spec.env.get(*key) {
                                     subset.insert(key.to_string(), val.clone());
                                 }
                             }
                             subset
                         },
                     },
                     fallbacks: ctx.graphics_stack.fallback_reasons.clone(),
                 };
                 let _ = session.write_effective_launch_config(&config);
            }
        }

        if let Some(session) = &ctx.session {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            let summary = crate::infra::logging::LaunchSummary {
                session_id: session.id.to_string(),
                app_id: ctx.app_id,
                app_name: ctx.app.as_ref().map(|a| a.name.clone()),
                runner_name: ctx.runner.as_ref().map(|r| r.name().to_string()),
                result,
                failing_stage,
                total_duration_ms,
                stage_durations_ms,
                timestamp,
                warnings: ctx.warnings.clone(),
                graphics_stack: Some(ctx.graphics_stack.clone()),
                verification: ctx.verification.clone(),
            };

            let _ = session.write_summary(&summary);

            // Add concise summary block to events log
            if let Some(logger) = &ctx.logger {
                 let mut metadata = HashMap::new();
                 if let Some(path) = &ctx.resolved_executable_path {
                     metadata.insert("exe".to_string(), path.to_string_lossy().to_string());
                 }
                 metadata.insert("backend".to_string(), ctx.graphics_stack.effective_backend.clone());
                 metadata.insert("d3d12_provider".to_string(), ctx.graphics_stack.effective_d3d12_provider.clone());
                 metadata.insert("gpu".to_string(), ctx.graphics_stack.effective_gpu.clone().unwrap_or_else(|| "default".to_string()));
                 metadata.insert("arch".to_string(), format!("{:?}", ctx.target_architecture).to_lowercase());
                 if let Some(spec) = &ctx.command_spec {
                     if let Some(overrides) = spec.env.get("WINEDLLOVERRIDES") {
                         metadata.insert("overrides".to_string(), overrides.clone());
                     }
                 }
                 metadata.insert("validation_passed".to_string(), ctx.warnings.is_empty().to_string());
                 metadata.insert("fallback_occurred".to_string(), (!ctx.graphics_stack.fallback_reasons.is_empty()).to_string());

                 if let Some(spec) = &ctx.command_spec {
                     if let Some(prefix) = spec.env.get("WINEPREFIX") {
                         metadata.insert("prefix".to_string(), prefix.clone());
                     }
                 }
                 if let Some(config) = &ctx.launcher_config {
                     metadata.insert("shared_prefix".to_string(), config.use_shared_compat_data.to_string());
                 }
                 metadata.insert("nvapi_requested".to_string(), ctx.graphics_stack.requested_nvapi.to_string());
                 metadata.insert("nvapi_exposed".to_string(), ctx.graphics_stack.effective_nvapi.to_string());
                 if let Some(ref detailed) = ctx.verification.detailed_status {
                     metadata.insert("verification_detailed".to_string(), detailed.clone());
                 }

                 if let Some(ref username) = ctx.verification.windows_username {
                      metadata.insert("windows_user".to_string(), username.clone());
                 }
                 metadata.insert("steam_client_exposed".to_string(), ctx.verification.steam_client_exposed.to_string());
                 metadata.insert("last_milestone".to_string(), ctx.verification.last_successful_startup_milestone.clone());
                 if !ctx.verification.dependency_families_detected.is_empty() {
                      metadata.insert("dependency_families".to_string(), ctx.verification.dependency_families_detected.join(", "));
                 }
                 if !ctx.verification.steam_runtime_milestone.is_empty() {
                      metadata.insert("steam_runtime_milestone".to_string(), ctx.verification.steam_runtime_milestone.clone());
                 }
                 if let Some(init) = ctx.verification.steam_api_initialized {
                      metadata.insert("steam_api_initialized".to_string(), init.to_string());
                 }
                 if let Some(own) = ctx.verification.steam_ownership_confirmed {
                      metadata.insert("steam_ownership_confirmed".to_string(), own.to_string());
                 }
                 if let Some(ref art) = ctx.verification.steam_client_artifact {
                      metadata.insert("steam_client_artifact".to_string(), art.clone());
                 }
                 if let Some(ref pfx) = ctx.verification.effective_game_wineprefix {
                      metadata.insert("effective_game_wineprefix".to_string(), pfx.clone());
                 }
                 if let Some(ref pfx) = ctx.verification.effective_steam_wineprefix {
                      metadata.insert("effective_steam_wineprefix".to_string(), pfx.clone());
                 }
                 if let Some(ref path) = ctx.verification.steam_client_install_path_exposed_to_game {
                      metadata.insert("steam_client_install_path_exposed_to_game".to_string(), path.clone());
                 }
                 if let Some(ref source) = ctx.verification.steam_client_install_path_source {
                      metadata.insert("steam_client_install_path_source".to_string(), source.clone());
                 }
                 metadata.insert("per_game_prefix_requested".to_string(), ctx.verification.per_game_prefix_requested.to_string());
                 metadata.insert("per_game_prefix_honored".to_string(), ctx.verification.per_game_prefix_honored.to_string());

                 metadata.insert("steam_running_before_launch".to_string(), ctx.verification.steam_running_before_launch.to_string());
                 metadata.insert("steam_auto_start_attempted".to_string(), ctx.verification.steam_auto_start_attempted.to_string());
                 metadata.insert("steam_auto_start_failed".to_string(), ctx.verification.steam_auto_start_failed.to_string());

                 let _ = logger.info("launch_summary_concise", "Concise launch summary recorded".to_string(), None, metadata);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    struct SuccessStage(&'static str);
    #[async_trait]
    impl PipelineStage for SuccessStage {
        fn name(&self) -> &str { self.0 }
        async fn execute(&self, _ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> { Ok(()) }
    }

    struct FailStage(&'static str);
    #[async_trait]
    impl PipelineStage for FailStage {
        fn name(&self) -> &str { self.0 }
        async fn execute(&self, _ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
            Err(LaunchError::new(LaunchErrorKind::Unknown, "failure"))
        }
    }

    #[tokio::test]
    async fn test_pipeline_order_and_success() {
        let mut pipeline = LaunchPipeline::new();
        pipeline.add_stage(Box::new(SuccessStage("stage1")));
        pipeline.add_stage(Box::new(SuccessStage("stage2")));

        let mut ctx = PipelineContext::new(0);
        assert!(pipeline.run(&mut ctx).await.is_ok());
    }

    #[tokio::test]
    async fn test_pipeline_short_circuit() {
        let mut pipeline = LaunchPipeline::new();
        pipeline.add_stage(Box::new(FailStage("stage1")));
        pipeline.add_stage(Box::new(SuccessStage("stage2")));

        let mut ctx = PipelineContext::new(0);
        let res = pipeline.run(&mut ctx).await;

        assert!(res.is_err());
        let err = res.unwrap_err();
        assert_eq!(err.stage_name, "stage1");
    }

    #[tokio::test]
    async fn test_pipeline_returned_error_context() {
        let mut pipeline = LaunchPipeline::new();
        pipeline.add_stage(Box::new(SuccessStage("stage1")));
        pipeline.add_stage(Box::new(FailStage("stage2")));

        let mut ctx = PipelineContext::new(0);
        let res = pipeline.run(&mut ctx).await;

        assert!(res.is_err());
        let err = res.unwrap_err();
        assert_eq!(err.stage_name, "stage2");
        assert!(err.inner.to_string().contains("failure"));
    }

    #[test]
    fn test_map_anyhow_error() {
        let err = anyhow::anyhow!("Permission denied: /tmp/pfx");
        let mapped = map_anyhow_error(err);
        assert_eq!(mapped.kind, LaunchErrorKind::Permission);

        let err = anyhow::anyhow!("file not found");
        let mapped = map_anyhow_error(err);
        assert_eq!(mapped.kind, LaunchErrorKind::GameData);

        let err = anyhow::anyhow!("random error");
        let mapped = map_anyhow_error(err);
        assert_eq!(mapped.kind, LaunchErrorKind::Unknown);
    }

    #[tokio::test]
    async fn test_pipeline_logging() {
        let mut pipeline = LaunchPipeline::new();
        pipeline.add_stage(Box::new(SuccessStage("test_stage")));

        let tmp = tempdir().unwrap();
        let session = LaunchSession::new(tmp.path());
        let logger = EventLogger::new(&session).unwrap();

        let mut ctx = PipelineContext::new(123);
        ctx.logger = Some(logger);

        pipeline.run(&mut ctx).await.unwrap();

        let content = std::fs::read_to_string(session.event_log_path()).unwrap();
        assert!(content.contains("launch_start"));
        assert!(content.contains("stage_start"));
        assert!(content.contains("test_stage"));
        assert!(content.contains("stage_success"));
        assert!(content.contains("launch_end"));
    }

    #[test]
    fn test_map_io_error_not_found() {
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
        let mapped = map_io_error(&err, None);
        assert_eq!(mapped.kind, LaunchErrorKind::GameData);
        assert!(mapped.message.contains("not found"));
        assert_eq!(mapped.context.get("io_kind").unwrap(), "NotFound");
    }

    #[test]
    fn test_map_io_error_permission_denied() {
        let err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let mapped = map_io_error(&err, None);
        assert_eq!(mapped.kind, LaunchErrorKind::Permission);
        assert!(mapped.message.contains("Access denied"));
    }

    #[test]
    fn test_map_io_error_lock_detected() {
        // Simulating a "Resource busy" or "locked" error which often maps to 'Other' or 'WouldBlock' in std::io
        let err = std::io::Error::new(std::io::ErrorKind::Other, "file is locked by another process");
        let mapped = map_io_error(&err, None);
        assert_eq!(mapped.kind, LaunchErrorKind::Process);
        assert!(mapped.message.contains("locked by another process"));
    }

    #[test]
    fn test_map_io_error_with_explicit_dup_info() {
        let err = std::io::Error::new(std::io::ErrorKind::Other, "generic error");
        let info = DuplicateInstanceInfo {
            detected: true,
            source: "lockfile".to_string(),
        };
        let mapped = map_io_error(&err, Some(&info));
        assert_eq!(mapped.kind, LaunchErrorKind::Process);
        assert!(mapped.message.contains("Ensure no other instance is running"));
        assert_eq!(mapped.context.get("duplicate_instance_detected").unwrap(), "true");
        assert_eq!(mapped.context.get("duplicate_detection_source").unwrap(), "lockfile");
    }

    #[test]
    fn test_detect_duplicate_instance_lockfile() {
        let tmp = tempfile::tempdir().unwrap();
        let lockfile = tmp.path().join(".aurelia_launch.lock");
        std::fs::write(&lockfile, "").unwrap();

        let mut ctx = PipelineContext::new(123);
        let mut spec = CommandSpec::default();
        spec.cwd = Some(tmp.path().to_path_buf());
        ctx.command_spec = Some(spec);

        let info = detect_duplicate_instance(&ctx);
        assert!(info.detected);
        assert_eq!(info.source, "lockfile");
    }

    #[tokio::test]
    async fn test_pipeline_structured_error_logging() {
        let mut pipeline = LaunchPipeline::new();
        pipeline.add_stage(Box::new(FailStage("fail_stage")));

        let tmp = tempdir().unwrap();
        let session = LaunchSession::new(tmp.path());
        let logger = EventLogger::new(&session).unwrap();

        let mut ctx = PipelineContext::new(456);
        ctx.logger = Some(logger);

        let _ = pipeline.run(&mut ctx).await;

        let content = std::fs::read_to_string(session.event_log_path()).unwrap();
        assert!(content.contains("stage_failure"));
        assert!(content.contains("fail_stage"));
        assert!(content.contains("error_kind"));
        assert!(content.contains("Unknown"));
        assert!(content.contains("failure"));
    }
}
