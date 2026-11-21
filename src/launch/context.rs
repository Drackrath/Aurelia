//! Launch pipeline context and evidence types.

use crate::core::config::LauncherConfig;
use crate::core::models::{LibraryGame, UserAppConfig};
use crate::launch::error::*;
use crate::launch::runners::{CommandSpec, Runner};
use crate::logging::{EventLogger, LaunchSession};
use crate::steam_client::LaunchInfo;
use std::collections::HashMap;

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
    pub graphics_stack_expected: String, // e.g. "DXVK v2.3, VKD3D-Proton v2.10"
    pub graphics_stack_evidence: Vec<String>, // e.g. ["DXVK: v2.3.1"]
    pub graphics_stack_confidence: String, // "low" | "medium" | "high"
    pub override_policy: String,         // e.g. "Native-only"
    pub dll_providers: HashMap<String, String>, // e.g. {"d3d11": "Runner", "d3d9": "GameLocal"}
    pub requested_backend: String,
    pub effective_backend: String,
    pub requested_d3d12_provider: String,
    pub effective_d3d12_provider: String,
    pub requested_nvapi: bool,
    pub effective_nvapi: bool,
    pub requested_gpu: Option<String>,
    pub effective_gpu: Option<String>,
    pub target_architecture: crate::core::models::ExecutableArchitecture,
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
    /// One-off launch-script override (e.g. `aurelia play --script <PATH>`) that
    /// takes precedence over the per-game config and the auto-detected dir script.
    pub launch_script_override: Option<std::path::PathBuf>,
    /// One-off request (e.g. `aurelia play --no-script`) to bypass all launch
    /// scripts for this launch.
    pub disable_launch_script: bool,
    /// Whether umu wrapping is active for this launch, resolved in
    /// `ResolveComponentsStage` and threaded into `LaunchContext`.
    pub use_umu: bool,
    /// The plugin-resolved absolute `umu-run` path (set when `use_umu`).
    pub umu_run: Option<std::path::PathBuf>,
    /// Run the game with real Steam integration
    pub steam_enabled: bool,
    /// `play --skip-shaders`: no replay.
    pub skip_shaders: bool,
    /// Shader replay progress for the caller.
    pub on_shader_event: Option<crate::launch::shaders::ShaderProgressFn>,

    pub resolved_install_dir: Option<std::path::PathBuf>,
    pub resolved_executable_path: Option<std::path::PathBuf>,
    pub executable_exists: bool,
    pub target_architecture: crate::core::models::ExecutableArchitecture,

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
    pub verification: crate::logging::LaunchVerification,
}

impl PipelineContext {
    /// Log to the session event log.
    pub fn log_event(
        &self,
        level: crate::logging::LogLevel,
        event_type: &str,
        message: String,
        stage: Option<String>,
        metadata: HashMap<String, String>,
    ) {
        if let Some(logger) = &self.logger {
            let _ = logger.log(level, event_type, message, stage, metadata);
        }
    }

    /// Info-level [`PipelineContext::log_event`].
    pub fn log_info(
        &self,
        event_type: &str,
        message: String,
        stage: Option<String>,
        metadata: HashMap<String, String>,
    ) {
        self.log_event(
            crate::logging::LogLevel::Info,
            event_type,
            message,
            stage,
            metadata,
        );
    }

    /// Error-level [`PipelineContext::log_event`].
    pub fn log_error(
        &self,
        event_type: &str,
        message: String,
        stage: Option<String>,
        metadata: HashMap<String, String>,
    ) {
        self.log_event(
            crate::logging::LogLevel::Error,
            event_type,
            message,
            stage,
            metadata,
        );
    }

    /// Snapshot pipeline state for a runner.
    pub fn to_runner_context(
        &mut self,
    ) -> std::result::Result<crate::launch::runners::LaunchContext, LaunchError> {
        let missing =
            |field: &str| LaunchError::new(LaunchErrorKind::Validation, format!("{field} missing"));
        Ok(crate::launch::runners::LaunchContext {
            app: self.app.as_ref().ok_or_else(|| missing("app"))?.clone(),
            launch_info: self
                .launch_info
                .as_ref()
                .ok_or_else(|| missing("launch_info"))?
                .clone(),
            launcher_config: self
                .launcher_config
                .as_ref()
                .ok_or_else(|| missing("launcher_config"))?
                .clone(),
            user_config: self.user_config.clone(),
            proton_path: self.proton_path.clone(),
            steam_enabled: self.steam_enabled,
            use_umu: self.use_umu,
            umu_run: self.umu_run.clone(),
            target_architecture: self.target_architecture,
            dll_resolutions: self.dll_resolutions.clone(),
            game_fixups: self.game_fixups.clone(),
            verification_ptr: &mut self.verification as *mut _,
        })
    }

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
            launch_script_override: None,
            disable_launch_script: false,
            use_umu: false,
            umu_run: None,
            steam_enabled: false,
            skip_shaders: false,
            on_shader_event: None,
            resolved_install_dir: None,
            resolved_executable_path: None,
            executable_exists: false,
            target_architecture: crate::core::models::ExecutableArchitecture::Unknown,
            runner: None,
            command_spec: None,
            child: None,
            session: None,
            logger: None,
            warnings: Vec::new(),
            graphics_stack: GraphicsStackInfo::default(),
            dll_resolutions: Vec::new(),
            game_fixups: crate::launch::fixups::GameFixups::default(),
            verification: crate::logging::LaunchVerification::default(),
        }
    }

    pub fn add_warning(&mut self, code: impl Into<String>, message: impl Into<String>) {
        let warning = CompatibilityWarning {
            code: code.into(),
            message: message.into(),
            context: HashMap::new(),
        };

        let mut metadata = HashMap::new();
        metadata.insert("warning_code".to_string(), warning.code.clone());
        metadata.insert("warning_message".to_string(), warning.message.clone());
        self.log_event(
            crate::logging::LogLevel::Warn,
            "compatibility_warning",
            warning.message.clone(),
            None,
            metadata,
        );

        self.warnings.push(warning);
    }
}
