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
    pub target_architecture: crate::models::ExecutableArchitecture,
    pub runner_path: String,
}

pub struct PreflightStage;

#[async_trait]
impl PipelineStage for PreflightStage {
    fn name(&self) -> &str { "Preflight" }

    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        let spec = ctx.command_spec.as_ref()
            .ok_or_else(|| LaunchError::new(LaunchErrorKind::Validation, "[Preflight] Command specification missing"))?;

        let mut checks = Vec::new();
        let runner_path = spec.program.to_string_lossy().to_string();

        let mut final_res: std::result::Result<(), LaunchError> = Ok(());

        // 1. Verify runner binary
        let runner_file = &spec.program;
        let mut check = PreflightCheck { name: "Runner Existence".into(), status: true, details: "OK".into() };
        if !runner_file.exists() {
            check.status = false;
            check.details = format!("Runner binary not found: {}", runner_file.display());
            final_res = Err(LaunchError::new(LaunchErrorKind::Runner, format!("[Preflight] {}", check.details))
                .with_context("runner_path", runner_path.clone()));
        } else if !runner_file.is_file() {
            check.status = false;
            check.details = format!("Runner path is not a file: {}", runner_file.display());
            final_res = Err(LaunchError::new(LaunchErrorKind::Runner, format!("[Preflight] {}", check.details))
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

                if game_exe_path.is_absolute() || (game_exe_path.components().count() > 1 && !game_exe.starts_with('-')) {
                     if !game_exe_path.exists() {
                         let mut resolved_path = game_exe_path.to_path_buf();
                         let mut resolved = false;
                         let mut fallback_used = false;

                         if let Some(app) = &ctx.app {
                             if let Some(install_path) = &app.install_path {
                                 let alt_path = Path::new(install_path).join(game_exe.replace('\\', "/"));
                                 if alt_path.exists() && alt_path.is_file() {
                                     resolved = true;
                                     fallback_used = true;
                                     resolved_path = alt_path;
                                 }
                             }
                         }

                         ctx.executable_exists = resolved;
                         if !resolved {
                             check.status = false;
                             check.details = format!("Game executable not found: {}", game_exe);

                             let mut err = LaunchError::new(LaunchErrorKind::GameData, format!("[Preflight] {}", check.details))
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
                             ctx.resolved_executable_path = Some(resolved_path);
                         }
                     } else if !game_exe_path.is_file() {
                          check.status = false;
                          check.details = format!("Game executable is not a file: {}", game_exe);
                          ctx.executable_exists = false;
                          final_res = Err(LaunchError::new(LaunchErrorKind::GameData, format!("[Preflight] {}", check.details))
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
                    final_res = Err(LaunchError::new(LaunchErrorKind::Environment, format!("[Preflight] {}", check.details))
                        .with_context("cwd", cwd.to_string_lossy()));
                } else if !cwd.is_dir() {
                    check.status = false;
                    check.details = format!("Working directory is not a directory: {}", cwd.display());
                    final_res = Err(LaunchError::new(LaunchErrorKind::Environment, format!("[Preflight] {}", check.details))
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
                    final_res = Err(LaunchError::new(LaunchErrorKind::Environment, format!("[Preflight] {}", check.details))
                        .with_context("wineprefix", prefix));
                } else if !prefix_path.is_dir() {
                    check.status = false;
                    check.details = format!("WINEPREFIX is not a directory: {}", prefix);
                    final_res = Err(LaunchError::new(LaunchErrorKind::Environment, format!("[Preflight] {}", check.details))
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
                    final_res = Err(LaunchError::new(LaunchErrorKind::Permission, format!("[Preflight] {}", check.details))
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
mod tests {
    use super::*;
    use crate::infra::runners::CommandSpec;
    use tempfile::tempdir;
    use std::fs;

    #[tokio::test]
    async fn test_preflight_missing_exe() {
        let mut ctx = PipelineContext::new(123);
        let mut spec = CommandSpec::default();
        spec.program = Path::new("/tmp/nonexistent_exe_12345").to_path_buf();
        ctx.command_spec = Some(spec);

        let stage = PreflightStage;
        let res = stage.execute(&mut ctx).await;

        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.message.contains("not found"));
        assert!(err.message.contains("[Preflight]"));
    }

    #[tokio::test]
    async fn test_preflight_missing_cwd() {
        let tmp = tempdir().unwrap();
        let exe = tmp.path().join("game.exe");
        fs::write(&exe, "dummy").unwrap();

        let mut ctx = PipelineContext::new(123);
        let mut spec = CommandSpec::default();
        spec.program = exe;
        spec.cwd = Some(tmp.path().join("missing_dir"));
        ctx.command_spec = Some(spec);

        let stage = PreflightStage;
        let res = stage.execute(&mut ctx).await;

        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.message.contains("Working directory does not exist"));
    }

    #[tokio::test]
    async fn test_preflight_missing_prefix() {
        let tmp = tempdir().unwrap();
        let exe = tmp.path().join("game.exe");
        fs::write(&exe, "dummy").unwrap();

        let mut ctx = PipelineContext::new(123);
        let mut spec = CommandSpec::default();
        spec.program = exe;
        spec.env.insert("WINEPREFIX".to_string(), tmp.path().join("missing_pfx").to_string_lossy().to_string());
        ctx.command_spec = Some(spec);

        let stage = PreflightStage;
        let res = stage.execute(&mut ctx).await;

        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.message.contains("WINEPREFIX does not exist"));
    }

    #[tokio::test]
    async fn test_preflight_is_not_directory() {
        let tmp = tempdir().unwrap();
        let exe = tmp.path().join("game.exe");
        fs::write(&exe, "dummy").unwrap();
        let not_a_dir = tmp.path().join("not_a_dir");
        fs::write(&not_a_dir, "dummy").unwrap();

        let mut ctx = PipelineContext::new(123);
        let mut spec = CommandSpec::default();
        spec.program = exe;
        spec.cwd = Some(not_a_dir);
        ctx.command_spec = Some(spec);

        let stage = PreflightStage;
        let res = stage.execute(&mut ctx).await;

        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.message.contains("is not a directory"));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_preflight_not_executable() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempdir().unwrap();
        let exe = tmp.path().join("game.exe");
        fs::write(&exe, "dummy").unwrap();
        let mut perms = fs::metadata(&exe).unwrap().permissions();
        perms.set_mode(0o644); // Not executable
        fs::set_permissions(&exe, perms).unwrap();

        let mut ctx = PipelineContext::new(123);
        let mut spec = CommandSpec::default();
        spec.program = exe;
        ctx.command_spec = Some(spec);

        let stage = PreflightStage;
        let res = stage.execute(&mut ctx).await;

        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.message.contains("is not executable"));
    }
}
