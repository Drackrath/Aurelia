//! Steam integration mode resolution.

use crate::launch::runners::LaunchContext;

/// Which Steam-integration mode a launch runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamMode {
    /// Bridge to the **host** Steam client via Proton's `lsteamclient` (full online
    /// Steamworks, Family-Shared licences). Chosen for `--steam` when a host Steam
    /// install is present. DLL overrides stay at Proton's defaults so `lsteamclient`
    /// loads builtin and bridges to the running client.
    HostBridge,
    /// Run against the Windows Steam runtime installed **inside Wine**
    /// (`aurelia steam-runtime install`): the game loads the real `steamclient.dll`
    /// and connects to the in-Wine `steam.exe` Aurelia starts. Provides Steam DRM
    /// (and offline Steamworks) with no host Steam install.
    InWineRuntime,
    /// No Steam client — the standalone fake-Steam trap. DRM-protected games can't run.
    Standalone,
}

/// Resolve the Steam-integration mode, reconciling the `--steam` flag with the
/// configurable [`SteamRuntimePolicy`] and which Steam installs are present.
///
/// The policy is the authoritative knob, resolved per-game first
/// (`aurelia config game <id> --steam-runtime …`) and, when that is `Auto`, from the
/// global default (`aurelia config steam-runtime-policy …`):
///   - `Enabled`  — always use the in-Wine Steam runtime (real `steamclient.dll` +
///     the `steam.exe` Aurelia starts), even when a host Steam exists.
///   - `Disabled` — never use the in-Wine runtime; `--steam` bridges to host Steam.
///   - `Auto`     — `--steam` prefers the host client and falls back to the in-Wine
///     runtime when no host Steam is installed (previously it assumed a
///     host Steam and silently ran DRM-protected games without DRM,
///     issue #3). Without `--steam`, standalone.
///
/// Returns the mode and a short source label recorded in launch diagnostics.
pub(super) fn resolve_steam_mode(ctx: &LaunchContext) -> (SteamMode, &'static str) {
    resolve_steam_mode_for(
        ctx.user_config.as_ref(),
        &ctx.launcher_config,
        ctx.steam_enabled,
    )
}

/// `resolve_steam_mode` without a launch context.
pub fn resolve_steam_mode_for(
    user_config: Option<&crate::core::models::UserAppConfig>,
    launcher_config: &crate::core::config::LauncherConfig,
    steam_enabled: bool,
) -> (SteamMode, &'static str) {
    use crate::core::models::SteamRuntimePolicy;

    // Per-game wins; `Auto` inherits global.
    let per_game = user_config
        .map(|c| c.steam_runtime_policy)
        .unwrap_or_default();
    let effective = match per_game {
        SteamRuntimePolicy::Auto => launcher_config.steam_runtime_policy,
        explicit => explicit,
    };

    let master_installed = || {
        crate::core::steam_host::get_master_steam_config()
            .steam_exe
            .is_some()
    };
    let host_installed = || crate::core::steam_host::host_steam_client_path().is_some();

    match effective {
        // Explicitly configured to use the in-Wine runtime.
        SteamRuntimePolicy::Enabled => {
            if master_installed() {
                (SteamMode::InWineRuntime, "policy_enabled")
            } else if steam_enabled && host_installed() {
                // Configured for the runtime but it isn't installed — honour --steam
                // by bridging to host Steam rather than silently disabling DRM.
                (SteamMode::HostBridge, "policy_enabled_host_fallback")
            } else {
                (SteamMode::Standalone, "policy_enabled_no_runtime")
            }
        }
        // In-Wine runtime forbidden: --steam uses host Steam (or standalone).
        SteamRuntimePolicy::Disabled => {
            if steam_enabled && host_installed() {
                (SteamMode::HostBridge, "host")
            } else if steam_enabled {
                (SteamMode::Standalone, "steam_flag_no_host")
            } else {
                (SteamMode::Standalone, "policy_disabled")
            }
        }
        // Auto: --steam prefers host, else the in-Wine runtime, else standalone.
        SteamRuntimePolicy::Auto => {
            if !steam_enabled {
                // Opportunistically bridge a running host Steam.
                if host_installed() && crate::core::steam_host::is_steam_running() {
                    tracing::info!("host Steam detected; bridging Steamworks");
                    (SteamMode::HostBridge, "host_running_opportunistic")
                } else {
                    (SteamMode::Standalone, "default")
                }
            } else if host_installed() {
                (SteamMode::HostBridge, "host")
            } else if master_installed() {
                (SteamMode::InWineRuntime, "steam_flag_no_host")
            } else {
                (SteamMode::Standalone, "steam_flag_no_steam")
            }
        }
    }
}
