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

#[cfg(test)]
#[path = "preflight_tests.rs"]
mod tests;
