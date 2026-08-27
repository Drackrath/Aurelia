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

impl LaunchContext {
    /// Run `f` against the pipeline's [`LaunchVerification`] record, if one is
    /// attached. Confines the raw-pointer dereference to this one audited spot.
    pub fn with_verification(&self, f: impl FnOnce(&mut crate::infra::logging::LaunchVerification)) {
        if self.verification_ptr.is_null() {
            return;
        }
        unsafe { f(&mut *self.verification_ptr) }
    }

    /// Resolve `(install_dir, executable, working_dir)` for the game.
    ///
    /// Errors if the game is not installed. The executable is resolved relative
    /// to the install dir unless it is absolute; the working dir honours an
    /// explicit `workingdir`, then the executable's parent, then the install dir.
    pub fn game_paths(
        &self,
    ) -> std::result::Result<(PathBuf, PathBuf, PathBuf), crate::launch::pipeline::LaunchError> {
        use crate::launch::pipeline::{LaunchError, LaunchErrorKind};
        let install_dir = PathBuf::from(self.app.install_path.clone().ok_or_else(|| {
            LaunchError::new(
                LaunchErrorKind::GameData,
                format!("game {} is not installed", self.app.app_id),
            )
        })?);

        let exe_rel = self.launch_info.executable.replace('\\', "/");
        let executable = if std::path::Path::new(&exe_rel).is_absolute() {
            PathBuf::from(&exe_rel)
        } else {
            install_dir.join(&exe_rel)
        };
        let working_dir = self
            .launch_info
            .workingdir
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|wd| install_dir.join(wd.replace('\\', "/")))
            .or_else(|| executable.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| install_dir.clone());

        Ok((install_dir, executable, working_dir))
    }
}

/// Point `STEAM_COMPAT_CLIENT_INSTALL_PATH` at Aurelia's fake-Steam trap so
/// Proton-style tools resolve a client install without a running Steam.
/// Returns the trap path for callers that record it.
pub fn insert_fake_steam_trap(
    env: &mut HashMap<String, String>,
) -> std::result::Result<PathBuf, crate::launch::pipeline::LaunchError> {
    use crate::launch::pipeline::{LaunchError, LaunchErrorKind};
    let config_dir = crate::core::config::config_dir().map_err(|e| {
        LaunchError::new(LaunchErrorKind::Environment, "failed to get config dir").with_source(e)
    })?;
    let fake_env = crate::core::utils::setup_fake_steam_trap(&config_dir).map_err(|e| {
        LaunchError::new(LaunchErrorKind::Permission, "failed to setup fake steam trap")
            .with_source(e)
    })?;
    env.insert(
        "STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string(),
        fake_env.to_string_lossy().to_string(),
    );
    Ok(fake_env)
}

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
