use std::path::{Path, PathBuf};
use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError, LaunchErrorKind};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PreflightCheck {
    pub name: String,
    pub status: bool,
    pub details: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PreflightReport {
    pub success: bool,
    pub checks: Vec<PreflightCheck>,
    pub target_architecture: crate::core::models::ExecutableArchitecture,
    pub runner_path: String,
}

pub struct PreflightStage;

/// Build a `LaunchError` whose message carries the standard `[Preflight]` prefix
/// in front of a check's `details`. Keeps the (kind, prefixed-message) pairing
/// consistent across every validation step below.
fn preflight_error(kind: LaunchErrorKind, details: &str) -> LaunchError {
    LaunchError::new(kind, format!("[Preflight] {}", details))
}

#[async_trait]
impl PipelineStage for PreflightStage {
    fn name(&self) -> &str { "Preflight" }

    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        let spec = ctx.command_spec.as_ref()
            .ok_or_else(|| LaunchError::new(LaunchErrorKind::Validation, "[Preflight] Command specification missing"))?;

        let mut checks = Vec::new();
        let runner_path = spec.program.to_string_lossy().to_string();

        let mut final_res: std::result::Result<(), LaunchError> = Ok(());

        // 1. Verify runner binary. When umu-launcher wraps the launch, `spec.program` is
        // the absolute plugin-resolved `umu-run` path, so the normal existence check
        // applies to it just like any other runner.
        let runner_file = &spec.program;
        let mut check = PreflightCheck { name: "Runner Existence".into(), status: true, details: "OK".into() };
        if !runner_file.exists() {
            check.status = false;
            check.details = format!("Runner binary not found: {}", runner_file.display());
            final_res = Err(preflight_error(LaunchErrorKind::Runner, &check.details)
                .with_context("runner_path", runner_path.clone()));
        } else if !runner_file.is_file() {
            check.status = false;
            check.details = format!("Runner path is not a file: {}", runner_file.display());
            final_res = Err(preflight_error(LaunchErrorKind::Runner, &check.details)
                .with_context("runner_path", runner_path.clone()));
        }
        checks.push(check);

        // 2. Verify target game executable
        if final_res.is_ok() {
            if let Some(game_exe) = spec.args.first() {
                let mut check = PreflightCheck { name: "Game Executable Existence".into(), status: true, details: "OK".into() };
                let game_exe_path = Path::new(game_exe);

                // Populate diagnostics in context
                if let Some(app) = &ctx.app {
                    ctx.resolved_install_dir = app.install_path.as_ref().map(PathBuf::from);
                }
                ctx.resolved_executable_path = Some(game_exe_path.to_path_buf());

                let looks_like_path = game_exe_path.is_absolute()
                    || (game_exe_path.components().count() > 1 && !game_exe.starts_with('-'));
                if looks_like_path {
                     if !game_exe_path.exists() {
                         let fallback_path = ctx.app.as_ref()
                             .and_then(|app| app.install_path.as_ref())
                             .map(|install_path| Path::new(install_path).join(game_exe.replace('\\', "/")))
                             .filter(|alt_path| alt_path.exists() && alt_path.is_file());
                         let fallback_used = fallback_path.is_some();

                         ctx.executable_exists = fallback_used;
                         if !fallback_used {
                             check.status = false;
                             check.details = format!("Game executable not found: {}", game_exe);

                             let mut err = preflight_error(LaunchErrorKind::GameData, &check.details)
                                .with_context("app_id", ctx.app_id.to_string())
                                .with_context("app_name", ctx.app.as_ref().map(|a| a.name.clone()).unwrap_or_default())
                                .with_context("game_exe", game_exe.to_string())
                                .with_context("resolved_path", game_exe_path.to_string_lossy())
                                .with_context("fallback_used", fallback_used.to_string());

                             if let Some(app) = &ctx.app {
                                 err = err.with_context("steam_install_dir", app.install_path.clone().unwrap_or_default());
                             }

                             final_res = Err(err);
                         } else {
                             ctx.resolved_executable_path = fallback_path;
                         }
                     } else if !game_exe_path.is_file() {
                          check.status = false;
                          check.details = format!("Game executable is not a file: {}", game_exe);
                          ctx.executable_exists = false;
                          final_res = Err(preflight_error(LaunchErrorKind::GameData, &check.details)
                            .with_context("game_exe", game_exe.to_string()));
                     } else {
                         ctx.executable_exists = true;
                     }
                }
                checks.push(check);
            }
        }

        // 3. Verify working directory
        if final_res.is_ok() {
            if let Some(cwd) = &spec.cwd {
                let mut check = PreflightCheck { name: "Working Directory".into(), status: true, details: "OK".into() };
                if !cwd.exists() {
                    check.status = false;
                    check.details = format!("Working directory does not exist: {}", cwd.display());
                    final_res = Err(preflight_error(LaunchErrorKind::Environment, &check.details)
                        .with_context("cwd", cwd.to_string_lossy()));
                } else if !cwd.is_dir() {
                    check.status = false;
                    check.details = format!("Working directory is not a directory: {}", cwd.display());
                    final_res = Err(preflight_error(LaunchErrorKind::Environment, &check.details)
                        .with_context("cwd", cwd.to_string_lossy()));
                }
                checks.push(check);
            }
        }

        // 4. Verify WINEPREFIX
        if final_res.is_ok() {
            if let Some(prefix) = spec.env.get("WINEPREFIX") {
                let mut check = PreflightCheck { name: "WINEPREFIX Existence".into(), status: true, details: "OK".into() };
                let prefix_path = Path::new(prefix);
                if !prefix_path.exists() {
                    check.status = false;
                    check.details = format!("WINEPREFIX does not exist: {}", prefix);
                    final_res = Err(preflight_error(LaunchErrorKind::Environment, &check.details)
                        .with_context("wineprefix", prefix));
                } else if !prefix_path.is_dir() {
                    check.status = false;
                    check.details = format!("WINEPREFIX is not a directory: {}", prefix);
                    final_res = Err(preflight_error(LaunchErrorKind::Environment, &check.details)
                        .with_context("wineprefix", prefix));
                }
                checks.push(check);
            }
        }

        // 5. Check runner executability
        #[cfg(unix)]
        if final_res.is_ok() {
            use std::os::unix::fs::PermissionsExt;
            let mut check = PreflightCheck { name: "Runner Executability".into(), status: true, details: "OK".into() };
            if let Ok(metadata) = std::fs::metadata(runner_file) {
                if metadata.is_file() && metadata.permissions().mode() & 0o111 == 0 {
                    check.status = false;
                    check.details = format!("Runner binary is not executable: {}", runner_file.display());
                    final_res = Err(preflight_error(LaunchErrorKind::Permission, &check.details)
                        .with_context("runner_path", runner_path.clone()));
                }
            }
            checks.push(check);
        }

        // 6. Steamworks library integrity — pre-spawn, only for launches that will
        // actually talk to a Steam client (InWineRuntime / HostBridge). The prefix
        // probe in `check_prefix_health` runs POST-spawn and only records warnings
        // after the game has already failed to start.
        if final_res.is_ok() {
            if let (Some(launcher_config), Some(prefix)) =
                (ctx.launcher_config.as_ref(), spec.env.get("WINEPREFIX"))
            {
                use crate::infra::runners::wine_tkg::{resolve_steam_mode_parts, SteamMode};
                let (steam_mode, _) =
                    resolve_steam_mode_parts(ctx.user_config.as_ref(), launcher_config, ctx.steam_enabled);

                if steam_mode != SteamMode::Standalone {
                    let mut check = PreflightCheck { name: "Steamworks Libraries".into(), status: true, details: "OK".into() };

                    // Game-side steam_api(64).dll: a corrupt/zero-byte copy is
                    // restored from a `.bak` sibling when one exists; otherwise
                    // warn clearly (no depot re-download is attempted here).
                    let mut game_dirs: Vec<PathBuf> = Vec::new();
                    if let Some(dir) = ctx.resolved_executable_path.as_ref().and_then(|e| e.parent()) {
                        game_dirs.push(dir.to_path_buf());
                    }
                    if let Some(install) = ctx.app.as_ref().and_then(|a| a.install_path.as_ref()) {
                        let install = PathBuf::from(install);
                        if !game_dirs.contains(&install) {
                            game_dirs.push(install);
                        }
                    }
                    // Emitted after the last `ctx` borrow below (add_warning
                    // needs `&mut ctx` while `spec`/`launcher_config` are still
                    // borrowed here).
                    let steam_api_notes = check_game_steam_api_libs(&game_dirs);

                    let prefix_path = Path::new(prefix);
                    match steam_mode {
                        SteamMode::InWineRuntime => {
                            let steam_dir = in_wine_steam_dir(prefix_path, ctx.user_config.as_ref(), launcher_config);
                            let missing = missing_steam_runtime_libs(&steam_dir);
                            if !missing.is_empty() {
                                check.status = false;
                                check.details = format!(
                                    "in-Wine Steam runtime libraries missing or corrupt under {}: {}. \
                                     Run `aurelia steam-runtime repair`.",
                                    steam_dir.display(),
                                    missing.join(", ")
                                );
                                final_res = Err(preflight_error(LaunchErrorKind::Environment, &check.details)
                                    .with_context("steam_dir", steam_dir.to_string_lossy()));
                            }
                        }
                        SteamMode::HostBridge => {
                            // umu wraps the launch, so `spec.program` is umu-run —
                            // the Proton tree is what PROTONPATH points at.
                            let runner_root = spec.env.get("PROTONPATH")
                                .map(PathBuf::from)
                                .unwrap_or_else(|| crate::core::utils::derive_runner_root(&spec.program));
                            if !lsteamclient_present(prefix_path, &runner_root) {
                                check.status = false;
                                check.details = format!(
                                    "lsteamclient.dll was found neither in the prefix ({}) nor in the \
                                     runner ({}) — the game cannot bridge to the host Steam client. \
                                     Use a Proton runner that ships lsteamclient, or install the in-Wine \
                                     Steam runtime (`aurelia steam-runtime install` / `aurelia steam-runtime repair`).",
                                    prefix_path.display(),
                                    runner_root.display()
                                );
                                final_res = Err(preflight_error(LaunchErrorKind::Environment, &check.details)
                                    .with_context("wineprefix", prefix)
                                    .with_context("runner_root", runner_root.to_string_lossy()));
                            }
                        }
                        SteamMode::Standalone => {}
                    }
                    for note in steam_api_notes {
                        ctx.add_warning("STEAM_API_INTEGRITY", note);
                    }
                    checks.push(check);
                }
            }
        }

        let report = PreflightReport {
            success: final_res.is_ok(),
            checks,
            target_architecture: ctx.target_architecture,
            runner_path,
        };

        if let Some(session) = &ctx.session {
            let _ = session.write_preflight_report(&report);
        }

        // 6. Architecture Hint & Context
        if let Some(logger) = &ctx.logger {
            let mut metadata = std::collections::HashMap::new();
            metadata.insert("runner_path".to_string(), report.runner_path.clone());
            metadata.insert("target_architecture".to_string(), format!("{:?}", report.target_architecture).to_lowercase());
            metadata.insert("success".to_string(), report.success.to_string());

            let event_type = if report.success { "preflight_success" } else { "preflight_failure" };
            let message = if report.success { "Preflight validation successful".to_string() } else { "Preflight validation failed".to_string() };

            let _ = logger.info(event_type, message, Some("Preflight".to_string()), metadata);
        }

        final_res
    }
}

/// Where the in-Wine Steam runtime's client libraries live for this launch:
/// the per-game deployment inside the game prefix, or the master Steam install
/// in shared mode (falling back to the canonical prefix location when steam.exe
/// hasn't been discovered).
fn in_wine_steam_dir(
    game_prefix: &Path,
    user_config: Option<&crate::core::models::UserAppConfig>,
    launcher_config: &crate::core::config::LauncherConfig,
) -> PathBuf {
    let mode = user_config
        .map(|c| c.steam_prefix_mode.clone())
        .unwrap_or(launcher_config.steam_prefix_mode.clone());
    match mode {
        crate::core::models::SteamPrefixMode::PerGame => {
            game_prefix.join("drive_c/Program Files (x86)/Steam")
        }
        crate::core::models::SteamPrefixMode::Shared => {
            let steam_cfg = crate::core::utils::get_master_steam_config();
            steam_cfg
                .steam_exe
                .as_ref()
                .and_then(|e| e.parent().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| steam_cfg.wine_prefix.join("drive_c/Program Files (x86)/Steam"))
        }
    }
}

/// Names of the in-Wine Steam runtime client libraries under `steam_dir` that a
/// Steamworks game loads and that are missing or fail the MZ-header check.
pub fn missing_steam_runtime_libs(steam_dir: &Path) -> Vec<String> {
    ["steam.exe", "steamclient.dll", "steamclient64.dll"]
        .into_iter()
        .filter(|name| !crate::launch::has_mz_header(&steam_dir.join(name)))
        .map(str::to_string)
        .collect()
}

/// Sweep `dirs` for the game-shipped Steamworks API DLLs (`steam_api.dll` /
/// `steam_api64.dll`) and verify each one present has a valid MZ header. A
/// corrupt DLL is restored from a `<name>.bak` sibling when a valid one exists;
/// otherwise a clear warning is produced — restoring from a depot re-download is
/// deliberately NOT attempted here. Returns a note per anomaly found/fixed.
pub fn check_game_steam_api_libs(dirs: &[PathBuf]) -> Vec<String> {
    let mut notes = Vec::new();
    for dir in dirs {
        for name in ["steam_api.dll", "steam_api64.dll"] {
            let dll = dir.join(name);
            if !dll.exists() || crate::launch::has_mz_header(&dll) {
                continue;
            }
            let bak = dir.join(format!("{name}.bak"));
            if crate::launch::has_mz_header(&bak) {
                match std::fs::copy(&bak, &dll) {
                    Ok(_) => notes.push(format!(
                        "restored corrupt {} from backup {}",
                        dll.display(),
                        bak.display()
                    )),
                    Err(e) => notes.push(format!(
                        "{} is corrupt (no MZ header) and restoring it from {} failed: {}",
                        dll.display(),
                        bak.display(),
                        e
                    )),
                }
            } else {
                notes.push(format!(
                    "{} is corrupt (no MZ header) and no valid .bak sibling exists — Steamworks \
                     init will fail; verify/reinstall the game files",
                    dll.display()
                ));
            }
        }
    }
    notes
}

/// Whether the `lsteamclient` host-Steam bridge is available to a launch:
/// already installed into the prefix (system32/syswow64, MZ-valid), or shipped
/// by the runner tree (Proton installs it into the prefix on first setup, so a
/// not-yet-set-up prefix is fine as long as the runner carries it).
pub fn lsteamclient_present(prefix: &Path, runner_root: &Path) -> bool {
    for sys in ["drive_c/windows/system32", "drive_c/windows/syswow64"] {
        if crate::launch::has_mz_header(&prefix.join(sys).join("lsteamclient.dll")) {
            return true;
        }
    }
    crate::compat::proton::UNIFIED_LIB_SUBDIRS.iter().any(|lib| {
        let base = runner_root.join(lib);
        // Modern Proton ships a PE `lsteamclient.dll` under the arch subdirs;
        // older builds a winelib `lsteamclient.dll.so` directly in the lib dir.
        crate::compat::proton::ARCH_SUBDIRS
            .iter()
            .any(|arch| base.join(arch).join("lsteamclient.dll").exists())
            || base.join("lsteamclient.dll").exists()
            || base.join("lsteamclient.dll.so").exists()
    })
}

#[cfg(test)]
#[path = "preflight_tests.rs"]
mod tests;
