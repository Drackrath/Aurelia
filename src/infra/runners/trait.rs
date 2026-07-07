use std::collections::HashMap;
use std::path::PathBuf;
use crate::core::models::{LibraryGame, UserAppConfig};
use crate::core::config::LauncherConfig;
use crate::steam_client::LaunchInfo;

#[derive(Debug, Clone)]
pub struct LaunchContext {
    pub app: LibraryGame,
    pub launch_info: LaunchInfo,
    pub launcher_config: LauncherConfig,
    pub user_config: Option<UserAppConfig>,
    pub proton_path: Option<String>,
    /// Run with real Steam integration
    pub steam_enabled: bool,
    /// Whether this launch is wrapped through the umu-launcher plugin (Proton via
    /// `umu-run`). Resolved in `ResolveComponentsStage`; the WineTkg runner spawns
    /// `umu_run` instead of a bare `proton run` when set.
    pub use_umu: bool,
    /// Absolute path to the plugin-resolved `umu-run` executable, populated when
    /// `use_umu` is set.
    pub umu_run: Option<std::path::PathBuf>,
    pub target_architecture: crate::core::models::ExecutableArchitecture,
    pub dll_resolutions: Vec<crate::launch::dll_provider_resolver::DllResolution>,
    /// Auto-resolved per-game fixups (env + DLL overrides) merged into the launch
    /// environment. Explicit user/per-game settings win over these on conflict.
    pub game_fixups: crate::launch::fixups::GameFixups,
    pub verification_ptr: *mut crate::infra::logging::LaunchVerification, // HACK: for Runner to write diagnostics
}

unsafe impl Send for LaunchContext {}
unsafe impl Sync for LaunchContext {}

#[derive(Debug, Clone, Default)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
}

#[async_trait::async_trait]
pub trait Runner: Send + Sync {
    fn name(&self) -> &str;
    async fn prepare_prefix(&self, ctx: &LaunchContext) -> std::result::Result<(), crate::launch::pipeline::LaunchError>;
    async fn build_env(&self, ctx: &LaunchContext) -> std::result::Result<HashMap<String, String>, crate::launch::pipeline::LaunchError>;
    async fn build_command(&self, ctx: &LaunchContext) -> std::result::Result<CommandSpec, crate::launch::pipeline::LaunchError>;
    fn launch(&self, spec: &CommandSpec) -> std::result::Result<std::process::Child, crate::launch::pipeline::LaunchError>;
}
