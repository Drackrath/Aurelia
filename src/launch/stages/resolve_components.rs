use crate::launch::{
    context::PipelineContext,
    error::{LaunchError, LaunchErrorKind},
    pipeline::PipelineStage,
};
use async_trait::async_trait;

use crate::launch::runners::{CommandSpec, LaunchContext, Runner};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
        let install_dir = ctx.install_dir()?;
        let crate::steam_client::GameCommand { program, args, cwd } =
            ctx.launch_info.command(install_dir);
        // Steam names most Mac builds by their bundle, which can't be exec'd.
        let bundled = if ctx.launch_info.target == crate::steam_client::LaunchTarget::NativeMacos {
            bundle_program(&program).await?
        } else {
            None
        };
        // LaunchServices starts apps in `/`, and Mac games locate their data through the bundle.
        let (program, cwd) = match bundled {
            Some(binary) => (binary, PathBuf::from("/")),
            None => (program, cwd),
        };
        // Only mutated on Linux.
        #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
        let mut env = self.build_env(ctx).await?;

        // 32-bit natives need scout runtime libraries.
        #[cfg(target_os = "linux")]
        if needs_scout_libs(&program, install_dir) {
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

/// The binary macOS runs for `program`, when `program` is an `.app` bundle.
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
    let not_found = || {
        LaunchError::new(
            LaunchErrorKind::GameData,
            format!("no executable macOS would run in {}", program.display()),
        )
    };
    let bundle = program.to_path_buf();
    let binary = tokio::task::spawn_blocking(move || {
        bundle_binary(&bundle, read_bundle_executable(&bundle))
    })
    .await
    .map_err(|e| not_found().with_source(anyhow::anyhow!(e)))?;
    Ok(Some(binary.ok_or_else(not_found)?))
}

/// What `Info.plist` says about a bundle's executable.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum BundleExecutable {
    /// No `Info.plist`, or a missing or empty `CFBundleExecutable`: macOS runs the binary
    /// named after the bundle.
    Unnamed,
    Named(String),
    /// A `CFBundleExecutable` that isn't a string, which macOS won't launch.
    Invalid,
}

/// Reads `bundle`'s `Info.plist` from disk on every call.
///
/// Not through `CFBundleCreate`: CF caches bundles per process, and `aurelia play` runs in
/// the long-lived daemon, so a game update that renames its executable would launch the old one.
#[cfg(target_os = "macos")]
fn read_bundle_executable(bundle: &Path) -> BundleExecutable {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    use core_foundation::url::CFURL;
    use core_foundation_sys::bundle::CFBundleCopyInfoDictionaryForURL;

    let info = CFURL::from_path(bundle, true).and_then(|url| {
        // SAFETY: `url` outlives the call, and a non-null result is ours to release (Create rule).
        unsafe {
            let raw = CFBundleCopyInfoDictionaryForURL(url.as_concrete_TypeRef());
            (!raw.is_null()).then(|| CFDictionary::<CFString, CFType>::wrap_under_create_rule(raw))
        }
    });
    let Some(value) = info
        .as_ref()
        .and_then(|info| info.find(CFString::from_static_string("CFBundleExecutable")))
    else {
        return BundleExecutable::Unnamed;
    };
    match value.downcast::<CFString>().map(|name| name.to_string()) {
        Some(name) if name.is_empty() => BundleExecutable::Unnamed,
        Some(name) => BundleExecutable::Named(name),
        None => BundleExecutable::Invalid,
    }
}

/// Only a Mac keeps macOS launch entries (`parse_launch_info_from_vdf`), so no bundle gets here.
#[cfg(not(target_os = "macos"))]
fn read_bundle_executable(_bundle: &Path) -> BundleExecutable {
    unreachable!("macOS launch entries exist only on a Mac")
}

/// The file `executable` names under `bundle`'s `Contents/MacOS`, if it resolves inside the
/// bundle, which rules out `/bin/sh` and escapes through `..` or symlinks.
///
/// Keeps the joined path rather than the resolved one: preflight only chmods a program under
/// the install path as spelled, and macOS also runs a symlinked binary by its own path.
fn bundle_binary(bundle: &Path, executable: BundleExecutable) -> Option<PathBuf> {
    let name = match executable {
        BundleExecutable::Unnamed => PathBuf::from(bundle.file_stem()?),
        BundleExecutable::Named(name) => PathBuf::from(name),
        BundleExecutable::Invalid => return None,
    };
    let binary = bundle.join("Contents/MacOS").join(name);
    let real = binary.canonicalize().ok()?;
    (real.is_file() && real.starts_with(bundle.canonicalize().ok()?)).then_some(binary)
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
