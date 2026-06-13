use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LaunchSessionId(String);

impl LaunchSessionId {
    pub fn generate() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_millis();
        let random: u32 = rand::random();
        Self(format!("{}-{:08x}", now, random))
    }
}

impl std::fmt::Display for LaunchSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LaunchSummary {
    pub session_id: String,
    pub app_id: u32,
    pub app_name: Option<String>,
    pub runner_name: Option<String>,
    pub result: LaunchResult,
    pub failing_stage: Option<String>,
    pub total_duration_ms: u128,
    pub stage_durations_ms: HashMap<String, u128>,
    pub timestamp: u64,
    #[serde(default)]
    pub warnings: Vec<crate::launch::pipeline::CompatibilityWarning>,
    #[serde(default)]
    pub graphics_stack: Option<crate::launch::pipeline::GraphicsStackInfo>,
    #[serde(default)]
    pub verification: LaunchVerification,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct LaunchVerification {
    pub status: String, // "verified", "uncertain", "failed_after_spawn", "not_verified"
    pub detailed_status: Option<String>,
    pub process_lifetime_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub log_growth_observed: bool,
    pub windows_username: Option<String>,
    pub windows_user_path: Option<String>,
    pub key_paths_detected: HashMap<String, bool>,
    pub steam_client_exposed: bool,
    pub last_successful_startup_milestone: String,
    pub dependency_families_detected: Vec<String>,
    pub steam_runtime_exe: Option<String>,
    pub steam_runtime_args: Vec<String>,
    pub steam_runtime_exit_code: Option<i32>,
    pub steam_runtime_lifetime_ms: Option<u64>,
    pub steam_runtime_milestone: String,
    pub steam_running_before_launch: bool,
    pub steam_auto_start_attempted: bool,
    pub steam_auto_start_failed: bool,
    pub steam_api_initialized: Option<bool>,
    pub steam_ownership_confirmed: Option<bool>,
    pub steam_client_artifact: Option<String>, // "local", "windows", "host"
    pub effective_game_wineprefix: Option<String>,
    pub effective_steam_wineprefix: Option<String>,
    pub steam_client_install_path_exposed_to_game: Option<String>,
    pub steam_client_install_path_source: Option<String>, // "real" vs "fake_trap"
    pub per_game_prefix_requested: bool,
    pub per_game_prefix_honored: bool,
    pub steam_runtime_policy: String,
    pub steam_runtime_source: String, // "default", "auto", "override"
    pub windows_steam_discovery_enabled: bool,
    pub log_head: Vec<String>,
    pub log_tail: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum LaunchResult {
    Success,
    Failure,
    Degraded, // Process spawned but policy was violated (e.g. WineD3D fallback when DXVK requested)
    Uncertain, // Process spawned but exited early or evidence is missing
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EffectiveEnv {
    pub runner_name: String,
    pub profile_id: Option<String>,
    pub profile_name: Option<String>,
    pub wine_dll_overrides: Option<String>,
    pub env_vars: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EffectiveLaunchConfig {
    pub session_id: String,
    pub app_id: u32,
    pub app_name: Option<String>,
    pub game: EffectiveGameConfig,
    pub runner: EffectiveRunnerConfig,
    pub settings: EffectiveSettingsConfig,
    pub command: EffectiveCommandConfig,
    pub fallbacks: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EffectiveGameConfig {
    pub install_dir: Option<PathBuf>,
    pub executable_path: Option<PathBuf>,
    pub executable_exists: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EffectiveRunnerConfig {
    pub name: Option<String>,
    pub root: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EffectiveSettingsConfig {
    pub requested_backend: String,
    pub effective_backend: String,
    pub requested_d3d12_provider: String,
    pub effective_d3d12_provider: String,
    pub requested_nvapi: bool,
    pub effective_nvapi: bool,
    pub requested_gpu: Option<String>,
    pub effective_gpu: Option<String>,
    pub target_architecture: crate::models::ExecutableArchitecture,
    pub dll_resolutions: Vec<crate::launch::dll_provider_resolver::DllResolution>,
    pub wine_dll_overrides: Option<String>,
    pub runtime_evidence: Option<crate::launch::pipeline::RuntimeEvidence>,
    pub env_propagation: Option<HashMap<String, bool>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EffectiveCommandConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env_subset: HashMap<String, String>,
}

pub struct LaunchSession {
    pub id: LaunchSessionId,
    pub created_at: SystemTime,
    pub log_dir: PathBuf,
}

impl LaunchSession {
    pub fn new(base_log_dir: &Path) -> Self {
        let id = LaunchSessionId::generate();
        let created_at = SystemTime::now();
        let log_dir = base_log_dir.join(id.to_string());

        Self {
            id,
            created_at,
            log_dir,
        }
    }

    pub fn event_log_path(&self) -> PathBuf {
        self.log_dir.join("events.jsonl")
    }

    pub fn summary_path(&self) -> PathBuf {
        self.log_dir.join("summary.json")
    }

    pub fn effective_env_path(&self) -> PathBuf {
        self.log_dir.join("effective_env.json")
    }

    pub fn effective_env_txt_path(&self) -> PathBuf {
        self.log_dir.join("effective_env.txt")
    }

    pub fn command_path(&self) -> PathBuf {
        self.log_dir.join("command.txt")
    }

    pub fn preflight_report_path(&self) -> PathBuf {
        self.log_dir.join("preflight_report.json")
    }

    pub fn stdout_path(&self) -> PathBuf {
        self.log_dir.join("stdout.log")
    }

    pub fn stderr_path(&self) -> PathBuf {
        self.log_dir.join("stderr.log")
    }

    /// Serialize `value` as pretty JSON and write it to `path`, ensuring the
    /// session log directory exists first. Shared by all JSON artifact writers.
    fn write_json_artifact<T: serde::Serialize + ?Sized>(&self, path: &Path, value: &T) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.log_dir)?;
        let content = serde_json::to_string_pretty(value)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn write_summary(&self, summary: &LaunchSummary) -> anyhow::Result<()> {
        self.write_json_artifact(&self.summary_path(), summary)
    }

    pub fn write_effective_env(&self, env: &EffectiveEnv) -> anyhow::Result<()> {
        let mut redacted_env = env.clone();
        redacted_env.env_vars = redact_environment(redacted_env.env_vars);
        self.write_json_artifact(&self.effective_env_path(), &redacted_env)
    }

    pub fn write_effective_env_txt(&self, env: &HashMap<String, String>) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.log_dir)?;
        let redacted = redact_environment(env.clone());
        let mut entries: Vec<_> = redacted.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));

        let mut content = String::new();
        for (key, value) in entries {
            content.push_str(&format!("{}={}\n", key, value));
        }
        std::fs::write(self.effective_env_txt_path(), content)?;
        Ok(())
    }

    pub fn write_command_artifact(&self, spec: &crate::infra::runners::CommandSpec) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.log_dir)?;
        let mut content = format!("Program: {}\n", spec.program.display());
        content.push_str(&format!("Args   : {}\n", spec.args.join(" ")));
        if let Some(cwd) = &spec.cwd {
            content.push_str(&format!("CWD    : {}\n", cwd.display()));
        }
        std::fs::write(self.command_path(), content)?;
        Ok(())
    }

    pub fn write_preflight_report<T: serde::Serialize>(&self, report: &T) -> anyhow::Result<()> {
        self.write_json_artifact(&self.preflight_report_path(), report)
    }

    pub fn write_dll_resolution_artifact(&self, resolutions: &[crate::launch::dll_provider_resolver::DllResolution]) -> anyhow::Result<()> {
        self.write_json_artifact(&self.log_dir.join("dll_resolution.json"), resolutions)
    }

    pub fn write_effective_launch_config(&self, config: &EffectiveLaunchConfig) -> anyhow::Result<()> {
        self.write_json_artifact(&self.log_dir.join("effective_launch_config.json"), config)
    }
}

pub fn redact_environment(env: HashMap<String, String>) -> HashMap<String, String> {
    super::redact_sensitive(env)
}

/// Build a `CompatibilityWarning` with the given code, message and context
/// entries. Centralizes the repeated struct construction in the sanity checks.
fn sanity_warning(
    code: &str,
    message: String,
    context: impl IntoIterator<Item = (String, String)>,
) -> crate::launch::pipeline::CompatibilityWarning {
    crate::launch::pipeline::CompatibilityWarning {
        code: code.to_string(),
        message,
        context: context.into_iter().collect(),
    }
}

pub fn check_environment_sanity(
    env_vars: &HashMap<String, String>,
    runner_name: &str,
    user_config: Option<&crate::models::UserAppConfig>,
) -> Vec<crate::launch::pipeline::CompatibilityWarning> {
    let mut warnings = Vec::new();

    // 1. Check for forced D3D defaults in WINEDLLOVERRIDES when they shouldn't be there
    if let Some(overrides) = env_vars.get("WINEDLLOVERRIDES") {
        let d3d_dlls = ["d3d9", "d3d11", "dxgi", "d3d12"];
        let is_baseline = user_config
            .map(|c| {
                c.graphics_layers.graphics_backend_policy != crate::models::GraphicsBackendPolicy::DXVK
                    && c.graphics_layers.d3d12_policy == crate::models::D3D12ProviderPolicy::Auto
                    && !c.graphics_layers.dxvk_enabled
                    && !c.graphics_layers.vkd3d_proton_enabled
                    && !c.graphics_layers.vkd3d_enabled
            })
            .unwrap_or(true);

        if is_baseline {
            for dll in d3d_dlls {
                if overrides.contains(&format!("{}=n", dll)) {
                    warnings.push(sanity_warning(
                        "SANITY_UNEXPECTED_OVERRIDE",
                        format!(
                            "WINEDLLOVERRIDES contains forced native override for '{}' in baseline mode. This may prevent the game from starting.",
                            dll
                        ),
                        [
                            ("dll".to_string(), dll.to_string()),
                            ("overrides".to_string(), overrides.clone()),
                        ],
                    ));
                }
            }
        }
    }

    // 2. Note if DXVK/VKD3D-related vars are absent when profile expects them
    if let Some(config) = user_config {
        if config.graphics_layers.dxvk_enabled {
            if !env_vars.contains_key("DXVK_LOG_LEVEL") && !env_vars.contains_key("DXVK_HUD") {
                warnings.push(sanity_warning(
                    "SANITY_DXVK_NO_DIAGNOSTICS",
                    "DXVK is enabled but no DXVK diagnostic variables (DXVK_LOG_LEVEL, DXVK_HUD) are set. Troubleshooting may be difficult if issues occur.".to_string(),
                    [],
                ));
            }
            if let Some(overrides) = env_vars.get("WINEDLLOVERRIDES") {
                if !overrides.contains("d3d11=n") && !overrides.contains("dxgi=n") && !overrides.contains("d3d9=n") && !overrides.contains("d3d8=n") && !overrides.contains("d3d10core=n") {
                    warnings.push(sanity_warning(
                        "SANITY_MISSING_DXVK_OVERRIDE",
                        "DXVK is enabled but WINEDLLOVERRIDES does not appear to contain native overrides for D3D11/DXGI/D3D9/D3D8/D3D10CORE.".to_string(),
                        [("overrides".to_string(), overrides.clone())],
                    ));
                }
            } else {
                warnings.push(sanity_warning(
                    "SANITY_MISSING_OVERRIDES_ENV",
                    "DXVK is enabled but WINEDLLOVERRIDES environment variable is missing.".to_string(),
                    [],
                ));
            }
        }

        if config.graphics_layers.vkd3d_proton_enabled || config.graphics_layers.vkd3d_enabled {
            if !env_vars.contains_key("VKD3D_DEBUG") && !env_vars.contains_key("VKD3D_CONFIG") {
                warnings.push(sanity_warning(
                    "SANITY_VKD3D_NO_DIAGNOSTICS",
                    "VKD3D is enabled but no VKD3D diagnostic variables (VKD3D_DEBUG, VKD3D_CONFIG) are set.".to_string(),
                    [],
                ));
            }
            if let Some(overrides) = env_vars.get("WINEDLLOVERRIDES") {
                if !overrides.contains("d3d12=n") {
                    warnings.push(sanity_warning(
                        "SANITY_MISSING_VKD3D_OVERRIDE",
                        "VKD3D is enabled but WINEDLLOVERRIDES does not appear to contain native overrides for D3D12.".to_string(),
                        [("overrides".to_string(), overrides.clone())],
                    ));
                }
            }
        }
    }

    // 3. Proton specific checks
    if runner_name.to_lowercase().contains("proton") && !env_vars.contains_key("STEAM_COMPAT_DATA_PATH") {
        warnings.push(sanity_warning(
            "SANITY_MISSING_PROTON_DATA_PATH",
            "Proton runner detected but STEAM_COMPAT_DATA_PATH is missing.".to_string(),
            [],
        ));
    }

    warnings
}
