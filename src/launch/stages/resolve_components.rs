use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError, LaunchErrorKind};

use std::collections::HashMap;
use std::path::PathBuf;
use crate::infra::runners::{Runner, LaunchContext, CommandSpec};

pub struct ResolveComponentsStage;

pub struct NativeRunner;

#[async_trait::async_trait]
impl Runner for NativeRunner {
    fn name(&self) -> &str { "Native" }
    async fn prepare_prefix(&self, _ctx: &LaunchContext) -> std::result::Result<(), LaunchError> { Ok(()) }
    async fn build_env(&self, ctx: &LaunchContext) -> std::result::Result<HashMap<String, String>, LaunchError> {
        let mut env = HashMap::new();
        env.insert("SteamAppId".to_string(), ctx.app.app_id.to_string());
        if let Some(config) = &ctx.user_config {
            env.extend(config.env_variables.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        Ok(env)
    }
    async fn build_command(&self, ctx: &LaunchContext) -> std::result::Result<CommandSpec, LaunchError> {
        let install_path = ctx.app.install_path.as_ref()
            .ok_or_else(|| LaunchError::new(LaunchErrorKind::GameData, "Install path missing"))?;

        let exe_rel = ctx.launch_info.executable.replace('\\', "/");
        let mut args: Vec<String> = ctx.launch_info.arguments.split_whitespace().map(str::to_string).collect();
        if let Some(config) = &ctx.user_config {
            args.extend(config.launch_options.split_whitespace().map(str::to_string));
        }
        let program = PathBuf::from(install_path).join(&exe_rel);
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

        // Preload emulator for steamless launch.
        #[cfg(target_os = "linux")]
        if let Some(lib) = crate::core::utils::resolve_steam_emulator(
            ctx.user_config.as_ref(),
            &ctx.launcher_config,
        ) {
            let existing = env.get("LD_PRELOAD").cloned().filter(|v| !v.is_empty());
            let value = match existing {
                Some(rest) => format!("{}:{}", lib.display(), rest),
                None => lib.display().to_string(),
            };
            env.insert("LD_PRELOAD".to_string(), value);
            // Goldberg reads the appid here.
            let appid_file = std::path::Path::new(install_path).join("steam_appid.txt");
            let _ = std::fs::write(&appid_file, ctx.app.app_id.to_string());
            tracing::info!("Steam emulator active: preloading {}", lib.display());
        } else if crate::core::utils::steam_emulator_requested(
            ctx.user_config.as_ref(),
            &ctx.launcher_config,
        ) {
            tracing::warn!(
                "Steam emulator enabled but libsteam_api.so not found at {}; launching without it",
                crate::core::utils::steam_emulator_lib_path(&ctx.launcher_config).display()
            );
        }

        Ok(CommandSpec {
            program,
            args,
            cwd: Some(PathBuf::from(install_path)),
            env,
        })
    }
    fn launch(&self, spec: &CommandSpec) -> std::result::Result<std::process::Child, LaunchError> {
        let spawn = |program: &std::path::Path, script: Option<&std::path::Path>| {
            let mut cmd = std::process::Command::new(program);
            if let Some(script) = script { cmd.arg(script); }
            cmd.args(&spec.args);
            if let Some(cwd) = &spec.cwd { cmd.current_dir(cwd); }
            cmd.envs(&spec.env);
            match session_log_file(spec, "AURELIA_STDOUT_LOG") {
                Some(file) => { cmd.stdout(file); }
                None => { cmd.stdout(std::process::Stdio::inherit()); }
            }
            match session_log_file(spec, "AURELIA_STDERR_LOG") {
                Some(file) => { cmd.stderr(file); }
                None => { cmd.stderr(std::process::Stdio::inherit()); }
            }
            cmd.spawn()
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
        result.map_err(|e| LaunchError::new(LaunchErrorKind::Process, "Native launch failed").with_source(anyhow::anyhow!(e)))
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
    if !file_head(program, 2).is_some_and(|h| h == *b"#!") {
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
    let Some(config) = &ctx.launcher_config else { return false };
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
    let Some(config) = &ctx.launcher_config else { return false };
    config.umu_enabled
        && config
            .game_configs
            .get(&ctx.app_id)
            .map(|g| g.runner == crate::core::config::GameRunner::Umu)
            .unwrap_or(false)
}

#[async_trait]
impl PipelineStage for ResolveComponentsStage {
    fn name(&self) -> &str { "ResolveComponents" }
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        use crate::infra::runners::{LuxtorpedaRunner, WineTkgRunner};
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
                let umu_run = crate::compat::umu::ensure_installed(custom_path).await.map_err(|e| {
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
                return Err(LaunchError::new(LaunchErrorKind::Validation, "LaunchInfo missing in ResolveComponentsStage"));
            };
            ctx.runner = Some(match info.target {
                LaunchTarget::NativeLinux => Box::new(NativeRunner) as Box<dyn Runner>,
                LaunchTarget::WindowsProton => Box::new(WineTkgRunner),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::{LibraryGame, UserAppConfig};
    use crate::steam_client::{LaunchInfo, LaunchTarget};

    #[tokio::test]
    async fn native_runner_appends_user_launch_options() {
        let mut user_config = UserAppConfig::default();
        user_config.launch_options = "-fullscreen --skip-intro".to_string();

        let ctx = LaunchContext {
            app: LibraryGame {
                app_id: 123,
                name: "Test".to_string(),
                install_path: Some("/tmp/game".to_string()),
                is_installed: true,
                playtime_forever_minutes: None,
                active_branch: "public".to_string(),
                update_available: false,
                update_queued: false,
                local_manifest_ids: HashMap::new(),
                is_owned: true,
                is_family_shared: false,
                online_required: None,
                platform: None,
                from_windows_steam: false,
            },
            launch_info: LaunchInfo {
                app_id: 123,
                id: "0".into(),
                description: "Test".into(),
                executable: "game.bin".into(),
                arguments: "-v".into(),
                workingdir: None,
                target: LaunchTarget::NativeLinux,
            },
            launcher_config: crate::core::config::LauncherConfig::default(),
            user_config: Some(user_config),
            proton_path: None,
            steam_enabled: false,
            use_umu: false,
            umu_run: None,
            target_architecture: Default::default(),
            dll_resolutions: Vec::new(),
            game_fixups: Default::default(),
            verification_ptr: std::ptr::null_mut(),
        };

        let spec = NativeRunner.build_command(&ctx).await.unwrap();
        assert_eq!(spec.args, vec!["-v", "-fullscreen", "--skip-intro"]);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn native_runner_preloads_emulator_when_active() {
        use crate::core::models::SteamEmulatorPolicy;

        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("game");
        std::fs::create_dir_all(&install).unwrap();
        let lib = dir.path().join("libsteam_api.so");
        std::fs::write(&lib, b"x").unwrap();

        let mut launcher_config = crate::core::config::LauncherConfig::default();
        launcher_config.steam_emulator = SteamEmulatorPolicy::Enabled;
        launcher_config.steam_emulator_path = Some(lib.to_string_lossy().into_owned());

        let ctx = LaunchContext {
            app: LibraryGame {
                app_id: 858710,
                name: "Test".to_string(),
                install_path: Some(install.to_string_lossy().into_owned()),
                is_installed: true,
                playtime_forever_minutes: None,
                active_branch: "public".to_string(),
                update_available: false,
                update_queued: false,
                local_manifest_ids: HashMap::new(),
                is_owned: true,
                is_family_shared: false,
                online_required: None,
                platform: Some("linux".to_string()),
                from_windows_steam: false,
            },
            launch_info: LaunchInfo {
                app_id: 858710,
                id: "0".into(),
                description: "Test".into(),
                executable: "game.bin".into(),
                arguments: String::new(),
                workingdir: None,
                target: LaunchTarget::NativeLinux,
            },
            launcher_config,
            user_config: None,
            proton_path: None,
            steam_enabled: false,
            use_umu: false,
            umu_run: None,
            target_architecture: Default::default(),
            dll_resolutions: Vec::new(),
            game_fixups: Default::default(),
            verification_ptr: std::ptr::null_mut(),
        };

        let spec = NativeRunner.build_command(&ctx).await.unwrap();
        assert!(spec.env.get("LD_PRELOAD").unwrap().contains("libsteam_api.so"));
        assert!(install.join("steam_appid.txt").is_file());
    }
}
