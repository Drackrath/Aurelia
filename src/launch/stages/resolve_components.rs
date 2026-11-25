use crate::launch::{
    context::PipelineContext,
    error::{LaunchError, LaunchErrorKind},
    pipeline::PipelineStage,
};
use async_trait::async_trait;

use crate::launch::runners::{CommandSpec, LaunchContext, Runner};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

pub struct ResolveComponentsStage;

pub struct NativeRunner;

#[async_trait::async_trait]
impl Runner for NativeRunner {
    fn name(&self) -> &str {
        "Native"
    }
    async fn prepare_prefix(&self, _ctx: &LaunchContext) -> std::result::Result<(), LaunchError> {
        Ok(())
    }
    async fn build_env(
        &self,
        ctx: &LaunchContext,
    ) -> std::result::Result<HashMap<String, String>, LaunchError> {
        let mut env = HashMap::new();
        env.insert("SteamAppId".to_string(), ctx.app.app_id.to_string());
        if let Some(config) = &ctx.user_config {
            env.extend(
                config
                    .env_variables
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone())),
            );
        }
        Ok(env)
    }
    async fn build_command(
        &self,
        ctx: &LaunchContext,
    ) -> std::result::Result<CommandSpec, LaunchError> {
        let install_path =
            ctx.app.install_path.as_ref().ok_or_else(|| {
                LaunchError::new(LaunchErrorKind::GameData, "Install path missing")
            })?;

        let exe_rel = ctx.launch_info.executable.replace('\\', "/");
        let args: Vec<String> = ctx
            .launch_info
            .arguments
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let program = PathBuf::from(install_path).join(&exe_rel);
        // Steam names most Mac builds by their bundle, which can't be exec'd.
        let bundled = if ctx.launch_info.target == crate::steam_client::LaunchTarget::NativeMacos {
            bundle_program(&program).await?
        } else {
            None
        };
        // LaunchServices starts apps in `/`, and Mac games locate their data through the bundle.
        let (program, cwd) = match bundled {
            Some(binary) => (binary, PathBuf::from("/")),
            None => (program, PathBuf::from(install_path)),
        };
        // Only mutated on Linux.
        #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
        let mut env = self.build_env(ctx).await?;

        // 32-bit natives need scout runtime libraries.
        #[cfg(target_os = "linux")]
        if needs_scout_libs(&program, std::path::Path::new(install_path)) {
            if let Some(libs) = scout_library_path() {
                let existing = env
                    .get("LD_LIBRARY_PATH")
                    .cloned()
                    .or_else(|| std::env::var("LD_LIBRARY_PATH").ok())
                    .filter(|v| !v.is_empty());
                let value = match existing {
                    Some(rest) => format!("{libs}:{rest}"),
                    None => libs,
                };
                env.insert("LD_LIBRARY_PATH".to_string(), value);
                tracing::info!("32-bit native game: using Steam scout runtime libraries");
            } else {
                tracing::warn!(
                    "32-bit native game but no Steam scout runtime found; \
                     libraries like libopenal.so.1 may be missing"
                );
            }
        }

        Ok(CommandSpec {
            program,
            args,
            cwd: Some(cwd),
            env,
        })
    }
    fn launch(&self, spec: &CommandSpec) -> std::result::Result<std::process::Child, LaunchError> {
        let spawn = |program: &std::path::Path, script: Option<&std::path::Path>| {
            let mut cmd = std::process::Command::new(program);
            if let Some(script) = script {
                cmd.arg(script);
            }
            cmd.args(&spec.args);
            if let Some(cwd) = &spec.cwd {
                cmd.current_dir(cwd);
            }
            cmd.envs(&spec.env);
            match session_log_file(spec, "AURELIA_STDOUT_LOG") {
                Some(file) => {
                    cmd.stdout(file);
                }
                None => {
                    cmd.stdout(crate::logging::relay::child_stdio());
                }
            }
            match session_log_file(spec, "AURELIA_STDERR_LOG") {
                Some(file) => {
                    cmd.stderr(file);
                }
                None => {
                    cmd.stderr(crate::logging::relay::child_stdio());
                }
            }
            cmd.spawn().map(crate::logging::relay::attach)
        };
        #[allow(unused_mut)]
        let mut result = spawn(&spec.program, None);
        // Shebang-less script: retry via sh.
        #[cfg(unix)]
        if result.as_ref().err().and_then(|e| e.raw_os_error()) == Some(libc::ENOEXEC) {
            tracing::info!(
                "{} is not executable directly (ENOEXEC); retrying via /bin/sh",
                spec.program.display()
            );
            result = spawn(std::path::Path::new("/bin/sh"), Some(&spec.program));
        }
        result.map_err(|e| {
            LaunchError::new(LaunchErrorKind::Process, "Native launch failed")
                .with_source(anyhow::anyhow!(e))
        })
    }
}

/// The `CFBundleExecutable` binary that runs `program`, when `program` is an `.app` bundle.
async fn bundle_program(program: &Path) -> std::result::Result<Option<PathBuf>, LaunchError> {
    let is_bundle = program
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
        && tokio::fs::metadata(program)
            .await
            .is_ok_and(|meta| meta.is_dir());
    if !is_bundle {
        return Ok(None);
    }
    let plist = program.join("Contents/Info.plist");
    let unreadable = || {
        LaunchError::new(
            LaunchErrorKind::GameData,
            format!("can't read CFBundleExecutable from {}", plist.display()),
        )
    };
    let output = tokio::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleExecutable", "raw"])
        .args(["-expect", "string", "-o", "-"])
        .arg(&plist)
        .output()
        .await
        .map_err(|e| unreadable().with_source(anyhow::anyhow!(e)))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(unreadable().with_context("plutil", stderr.trim()));
    }
    let binary = bundle_binary(program, &output.stdout).ok_or_else(|| {
        unreadable().with_context(
            "CFBundleExecutable",
            String::from_utf8_lossy(&output.stdout),
        )
    })?;
    Ok(Some(binary))
}

/// The `Contents/MacOS` binary that plutil's raw `CFBundleExecutable` output names.
///
/// `None` unless the name is a single path component, which keeps the binary inside
/// the bundle, where preflight's lexical install-dir check allows a chmod.
fn bundle_binary(bundle: &Path, cf_bundle_executable: &[u8]) -> Option<PathBuf> {
    let raw = cf_bundle_executable
        .strip_suffix(b"\n")
        .unwrap_or(cf_bundle_executable);
    let mut parts = Path::new(std::str::from_utf8(raw).ok()?).components();
    match (parts.next(), parts.next()) {
        (Some(Component::Normal(name)), None) => Some(bundle.join("Contents/MacOS").join(name)),
        _ => None,
    }
}

/// First `n` bytes of the file.
#[cfg(target_os = "linux")]
fn file_head(path: &std::path::Path, n: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    std::fs::File::open(path).ok()?.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// Is the file a 32-bit ELF?
#[cfg(target_os = "linux")]
fn is_elf32(path: &std::path::Path) -> bool {
    file_head(path, 5).is_some_and(|h| h[..4] == *b"\x7fELF" && h[4] == 1)
}

/// Does this launch need scout runtime libraries?
///
/// True for a 32-bit ELF program, or a shell-script program (like GoldSrc's
/// `hl.sh`) whose install directory carries a 32-bit ELF at the top level.
#[cfg(target_os = "linux")]
fn needs_scout_libs(program: &std::path::Path, install_dir: &std::path::Path) -> bool {
    if is_elf32(program) {
        return true;
    }
    if file_head(program, 2).is_none_or(|h| h != *b"#!") {
        return false;
    }
    std::fs::read_dir(install_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .take(200)
        .any(|e| is_elf32(&e.path()))
}

/// The scout runtime's library search path, when installed.
#[cfg(target_os = "linux")]
fn scout_library_path() -> Option<String> {
    let runtime = crate::library::steam_root_candidates()
        .into_iter()
        .map(|root| root.join("ubuntu12_32/steam-runtime"))
        .find(|p| p.is_dir())?;
    let joined = [
        "lib/i386-linux-gnu",
        "usr/lib/i386-linux-gnu",
        "lib/x86_64-linux-gnu",
        "usr/lib/x86_64-linux-gnu",
    ]
    .iter()
    .map(|d| runtime.join(d))
    .filter(|p| p.is_dir())
    .map(|p| p.to_string_lossy().into_owned())
    .collect::<Vec<_>>()
    .join(":");
    (!joined.is_empty()).then_some(joined)
}

/// Open the session log named by `key`.
fn session_log_file(spec: &CommandSpec, key: &str) -> Option<std::fs::File> {
    let path = std::path::PathBuf::from(spec.env.get(key)?);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::File::create(&path).ok()
}

/// Whether this launch should be routed through the luxtorpeda plugin: either a one-off
/// `--native-engine` override, or the game is pinned to it while the feature is enabled.
fn wants_luxtorpeda(ctx: &PipelineContext) -> bool {
    if ctx.force_native_engine {
        return true;
    }
    let Some(config) = &ctx.launcher_config else {
        return false;
    };
    config.luxtorpeda_enabled
        && config
            .game_configs
            .get(&ctx.app_id)
            .map(|g| g.runner == crate::core::config::GameRunner::Luxtorpeda)
            .unwrap_or(false)
}

/// Whether this launch should be wrapped through the umu-launcher plugin: either a
/// one-off `--umu` override, or the game is pinned to it while the feature is enabled.
fn wants_umu(ctx: &PipelineContext) -> bool {
    if ctx.force_umu {
        return true;
    }
    let Some(config) = &ctx.launcher_config else {
        return false;
    };
    config.umu_enabled
        && config
            .game_configs
            .get(&ctx.app_id)
            .map(|g| g.runner == crate::core::config::GameRunner::Umu)
            .unwrap_or(false)
}

#[async_trait]
impl PipelineStage for ResolveComponentsStage {
    fn name(&self) -> &str {
        "ResolveComponents"
    }
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        use crate::launch::runners::{LuxtorpedaRunner, WineTkgRunner};
        use crate::steam_client::LaunchTarget;

        if ctx.runner.is_none() {
            // The luxtorpeda native-engine plugin is Linux-only. Route through it when the
            // launch was explicitly forced (`--native-engine`) or the game is pinned to it
            // and the feature is enabled.
            if cfg!(target_os = "linux") && wants_luxtorpeda(ctx) {
                ctx.runner = Some(Box::new(LuxtorpedaRunner) as Box<dyn Runner>);
                return Ok(());
            }

            // The umu-launcher plugin *wraps* Proton rather than replacing the runner:
            // it is Linux-only and, when active, we keep the normal Proton/Wine runner
            // but resolve the `umu-run` entry point (downloading on first use) so the
            // runner spawns the game through umu. A one-off `--umu` on a non-Linux host
            // is a hard error, matching the `--native-engine` guard.
            if ctx.force_umu && !cfg!(target_os = "linux") {
                return Err(LaunchError::new(
                    LaunchErrorKind::Validation,
                    "umu-launcher (`--umu`) is only available on Linux",
                ));
            }
            if cfg!(target_os = "linux") && wants_umu(ctx) {
                let custom = ctx
                    .launcher_config
                    .as_ref()
                    .and_then(|c| c.umu_path.clone());
                let custom_path = custom.as_deref().map(std::path::Path::new);
                let umu_run = crate::compat::umu::ensure_installed(custom_path)
                    .await
                    .map_err(|e| {
                        LaunchError::new(
                            LaunchErrorKind::Runner,
                            format!("failed to resolve the umu-launcher plugin: {e:#}"),
                        )
                        .with_source(e)
                    })?;
                ctx.use_umu = true;
                ctx.umu_run = Some(umu_run);
            }

            let Some(info) = &ctx.launch_info else {
                return Err(LaunchError::new(
                    LaunchErrorKind::Validation,
                    "LaunchInfo missing in ResolveComponentsStage",
                ));
            };
            ctx.runner = Some(match info.target {
                LaunchTarget::NativeLinux | LaunchTarget::NativeMacos => {
                    Box::new(NativeRunner) as Box<dyn Runner>
                }
                LaunchTarget::WindowsProton => Box::new(WineTkgRunner),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
