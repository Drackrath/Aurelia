//! `SteamClient` methods: Steam process control, wine-prefix helpers, headless cfg, ad-hoc launch.
//!
//! Split out of `steam_client.rs` for readability; the struct, shared imports
//! and free helpers live in the parent module (in scope via `use super::*`).
use super::*;

impl SteamClient {
    /// Whether the desktop Steam client appears to be running.
    ///
    /// The running client caches each game's appmanifest at startup, so changes we
    /// make on disk (e.g. enabling a DLC) aren't visible to games until Steam
    /// re-reads them — which it does on restart.
    #[cfg(target_os = "windows")]
    pub fn steam_is_running() -> bool {
        read_steam_registry("SteamPID")
            .and_then(|v| {
                let v = v.trim();
                v.strip_prefix("0x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| v.parse::<u32>().ok())
            })
            .is_some_and(|pid| pid != 0)
    }

    #[cfg(not(target_os = "windows"))]
    pub fn steam_is_running() -> bool {
        false
    }

    /// Ask the desktop Steam client to shut down, and wait for it to fully exit.
    /// Windows only. Editing appmanifests is only reliable while Steam is stopped,
    /// because Steam flushes its in-memory app state to disk on exit.
    #[cfg(target_os = "windows")]
    pub fn shutdown_steam() -> Result<()> {
        if !SteamClient::steam_is_running() {
            return Ok(());
        }
        let exe = steam_exe_path().context("could not locate steam.exe to stop Steam")?;
        Command::new(&exe)
            .arg("-shutdown")
            .spawn()
            .context("failed to signal Steam shutdown")?;
        for _ in 0..60 {
            if !SteamClient::steam_is_running() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        bail!("Steam did not shut down within 30s")
    }

    /// Start the desktop Steam client (Windows only).
    ///
    /// Launched with `-silent` so it starts minimized to the system tray rather
    /// than popping its window to the foreground — Aurelia only restarts Steam to
    /// have it re-read state (e.g. after a DLC/move change), not to bring it up.
    #[cfg(target_os = "windows")]
    pub fn start_steam() -> Result<()> {
        let exe = steam_exe_path().context("could not locate steam.exe to start Steam")?;
        Command::new(&exe)
            .arg("-silent")
            .spawn()
            .context("failed to start Steam")?;
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    pub fn shutdown_steam() -> Result<()> {
        bail!("automatic Steam control is only supported on Windows")
    }

    #[cfg(not(target_os = "windows"))]
    pub fn start_steam() -> Result<()> {
        bail!("automatic Steam control is only supported on Windows")
    }

    /// Stop a game previously launched by `aurelia play`. Looks up the launch
    /// record Aurelia wrote (PID, and for a per-game Proton/Wine launch the
    /// WINEPREFIX) and terminates the process tree, then clears the record.
    ///
    /// Returns the resolved record on success. Fails if Aurelia has no record of
    /// the game running — e.g. it was started directly through Steam rather than
    /// `aurelia play`.
    /// Stop a game previously launched by `aurelia play`. With `force`, processes
    /// are killed immediately (SIGKILL / `taskkill /F`); otherwise they are first
    /// asked to exit (SIGTERM) so the game can shut down and save cleanly.
    pub fn stop_game(app_id: u32, force: bool) -> Result<crate::running::RunningGame> {
        let record = crate::running::load(app_id).ok_or_else(|| {
            anyhow!("app {app_id} is not running (no launch was recorded by Aurelia)")
        })?;

        // A Proton/Wine game runs as wine processes inside its WINEPREFIX; killing
        // the recorded runner PID alone can leave them behind. Sweep the per-game
        // prefix too when we recorded one (never the shared master prefix).
        #[cfg(unix)]
        if let Some(prefix) = record.wineprefix.as_deref() {
            Self::kill_wine_processes_in_prefix(prefix, force);
        }

        // Proton re-parents the game's processes (steam.exe shim, the game exe,
        // wineserver) away from the runner we spawned, so killing the recorded PID
        // tree alone leaves them running — and in the default shared-prefix mode no
        // wineprefix is recorded to sweep. Every one of those processes carries
        // STEAM_COMPAT_APP_ID=<app_id> in its environment, so use that as the
        // authoritative way to find and stop the whole game.
        #[cfg(unix)]
        Self::kill_processes_for_app(app_id, force);

        kill_process_tree(record.pid, force);
        crate::running::clear(app_id);
        Ok(record)
    }

    /// Terminate every process belonging to `app_id`, identified by
    /// `STEAM_COMPAT_APP_ID=<app_id>` in its environment. This catches Proton's
    /// re-parented game/steam.exe/wineserver processes that aren't in the spawned
    /// runner's tree. The master Steam client never carries a game's app id, so it
    /// is not affected.
    #[cfg(unix)]
    pub fn kill_processes_for_app(app_id: u32, force: bool) {
        let needle = format!("STEAM_COMPAT_APP_ID={app_id}");
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };

        Self::scan_proc_pids(|pid_path, pid_str| {
            let environ = match std::fs::read(pid_path.join("environ")) {
                Ok(b) => b,
                Err(_) => return None,
            };
            // environ is NUL-separated `KEY=VALUE` entries; match one exactly so
            // app id 945360 never matches 9453600.
            let matches = environ
                .split(|&b| b == 0)
                .any(|entry| entry == needle.as_bytes());
            if !matches {
                return None;
            }

            if let Ok(pid) = pid_str.parse::<i32>() {
                unsafe {
                    libc::kill(pid, signal);
                }
            }
            // Never short-circuit: sweep every matching process.
            None::<()>
        });
    }

    /// Invoke `f` once per numeric `/proc/<pid>` directory, passing its path and
    /// the (still string) pid name. Returns early with `f`'s value the first time
    /// it yields `Some`; returns `None` if `/proc` is unreadable or no entry
    /// matched. Centralizes the directory scan + numeric-pid filter shared by the
    /// prefix-scanning helpers so each caller only expresses its own match logic.
    #[cfg(unix)]
    fn scan_proc_pids<T>(mut f: impl FnMut(&Path, &str) -> Option<T>) -> Option<T> {
        let proc_dir = std::fs::read_dir("/proc").ok()?;
        for entry in proc_dir.flatten() {
            let pid_path = entry.path();
            let Some(pid_str) = pid_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !pid_str.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if let Some(out) = f(&pid_path, pid_str) {
                return Some(out);
            }
        }
        None
    }

    /// Terminate every wine process running inside `wineprefix` (identified by the
    /// prefix path appearing in the process environment). Used to stop a
    /// Proton/Wine game whose processes outlive the runner we spawned. Only call
    /// this for a per-game prefix — the shared master prefix also hosts Steam.
    #[cfg(unix)]
    pub fn kill_wine_processes_in_prefix(wineprefix: &Path, force: bool) {
        let prefix_str = wineprefix.to_string_lossy().to_string();
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };

        Self::scan_proc_pids(|pid_path, pid_str| {
            let environ = match std::fs::read(pid_path.join("environ")) {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(_) => return None,
            };
            if !environ.contains(&prefix_str) {
                return None;
            }

            if let Ok(pid) = pid_str.parse::<i32>() {
                unsafe {
                    libc::kill(pid, signal);
                }
            }
            // Never short-circuit: sweep every matching process.
            None::<()>
        });
    }

    pub fn kill_steam_in_prefix(wineprefix: &Path) {
        #[cfg(unix)]
        {
            let prefix_str = wineprefix.to_string_lossy().to_string();

            Self::scan_proc_pids(|pid_path, pid_str| {
                let cmdline = match std::fs::read(pid_path.join("cmdline")) {
                    Ok(b) => String::from_utf8_lossy(&b).replace('\0', " "),
                    Err(_) => return None,
                };
                // Kill steam.exe and steamwebhelper.exe processes in this prefix
                if !cmdline.to_lowercase().contains("steam.exe")
                    && !cmdline.to_lowercase().contains("steamwebhelper.exe")
                {
                    return None;
                }

                let environ = match std::fs::read(pid_path.join("environ")) {
                    Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                    Err(_) => return None,
                };
                if !environ.contains(&prefix_str) {
                    return None;
                }

                if let Ok(pid) = pid_str.parse::<i32>() {
                    unsafe {
                        libc::kill(pid, libc::SIGTERM);
                    }
                }
                // Never short-circuit: sweep every matching process.
                None::<()>
            });
        }
        #[cfg(not(unix))]
        {
            let _ = wineprefix;
        }
    }

    /// Scans /proc to find a wine process running steam.exe inside the given WINEPREFIX.
    pub fn is_steam_running_in_prefix(wineprefix: &Path) -> bool {
        #[cfg(unix)]
        {
            let prefix_str = wineprefix.to_string_lossy().to_string();

            Self::scan_proc_pids(|pid_path, _pid_str| {
                // Must have steam.exe in cmdline
                let cmdline = match std::fs::read(pid_path.join("cmdline")) {
                    Ok(b) => b,
                    Err(_) => return None,
                };
                let cmdline_str = String::from_utf8_lossy(&cmdline).replace('\0', " ");
                if !cmdline_str.to_lowercase().contains("steam.exe") {
                    return None;
                }

                // Must have our WINEPREFIX in its environment
                let environ = match std::fs::read(pid_path.join("environ")) {
                    Ok(b) => b,
                    Err(_) => return None,
                };
                let environ_str = String::from_utf8_lossy(&environ);
                environ_str.contains(&prefix_str).then_some(true)
            })
            .unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            let _ = wineprefix;
            false
        }
    }

    /// Writes a steam.cfg into the Steam directory that minimises UI on startup.
    pub fn write_headless_steam_cfg(steam_dir: &Path) {
        let cfg_path = steam_dir.join("steam.cfg");
        // Only write if not already present to avoid overwriting user config
        if cfg_path.exists() {
            return;
        }
        let content = "\
BootStrapperForceSelfUpdate=disable
SteamDefaultDialog=Friends
NoSavePersonalInfo=1
";
        let _ = std::fs::write(&cfg_path, content);
    }

    /// Launch a Windows game's executable directly, with no Proton/Wine layer.
    /// Used on Windows hosts (and when `--windows` is forced), where the game's
    /// native `.exe` runs without a compatibility layer.
    pub(crate) async fn spawn_windows_native(
        &self,
        app: &LibraryGame,
        launch_info: &LaunchInfo,
        user_config: Option<&crate::models::UserAppConfig>,
    ) -> Result<std::process::Child> {
        let install_dir = match app.install_path.as_ref().map(PathBuf::from) {
            Some(p) if p.exists() => p,
            _ => self.install_root_for_app(app.app_id).await?,
        };

        // Steam VDF stores Windows paths with backslashes; normalize for the host separator.
        let exe_relative = launch_info.executable.replace('\\', "/");
        let executable = install_dir.join(&exe_relative);
        let mut args = split_args(&launch_info.arguments);

        if let Some(config) = user_config {
            if !config.launch_options.trim().is_empty() {
                args.extend(split_args(&config.launch_options));
            }
        }

        let game_working_dir: PathBuf = launch_info
            .workingdir
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|wd| install_dir.join(wd.replace('\\', "/")))
            .or_else(|| executable.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| install_dir.clone());

        // Standard Steam identity fallback so the game can resolve its app id.
        let app_id_path = game_working_dir.join("steam_appid.txt");
        std::fs::write(&app_id_path, app.app_id.to_string()).unwrap_or_default();

        let mut cmd = Command::new(&executable);
        cmd.args(&args);
        cmd.current_dir(&game_working_dir);
        cmd.env("SteamAppId", app.app_id.to_string());

        if let Some(config) = user_config {
            for (key, val) in &config.env_variables {
                cmd.env(key, val);
            }
        }

        tracing::info!(
            "Launching game (Native Windows): {:?} with args {:?}",
            executable,
            args
        );
        cmd.spawn()
            .with_context(|| format!("failed to spawn windows game {}", executable.display()))
    }

    pub(crate) async fn spawn_game_process(
        &self,
        app: &LibraryGame,
        launch_info: &LaunchInfo,
        proton_path: Option<&str>,
        launcher_config: &crate::config::LauncherConfig,
        user_config: Option<&crate::models::UserAppConfig>,
        force_native_engine: bool,
        steam_enabled: bool,
    ) -> Result<std::process::Child> {
        use crate::launch::pipeline::{LaunchPipeline, PipelineContext};
        use crate::infra::logging::{LaunchSession, EventLogger};

        let mut ctx = PipelineContext::new(app.app_id);
        ctx.app = Some(app.clone());
        ctx.launch_info = Some(launch_info.clone());
        ctx.launcher_config = Some(launcher_config.clone());
        ctx.user_config = user_config.cloned();
        ctx.proton_path = proton_path.map(|s| s.to_string());
        ctx.force_native_engine = force_native_engine;
        ctx.steam_enabled = steam_enabled;

        if let Ok(config_dir) = crate::config::config_dir() {
            let session = LaunchSession::new(&config_dir.join("logs"));
            if let Ok(logger) = EventLogger::new(&session) {
                ctx.session = Some(session);
                ctx.logger = Some(logger);
            }
        }

        let pipeline = LaunchPipeline::with_default_stages();
        pipeline.run(&mut ctx).await
            .map_err(|e| anyhow!(e))?;

        ctx.child.ok_or_else(|| anyhow!("Pipeline finished without spawning a process"))
    }

    /// Internal legacy ad-hoc launch path.
    /// TODO: Remove once NativeRunner is implemented. (Ref: issue #1)
    pub async fn internal_legacy_launch_adhoc(
        &self,
        app: &LibraryGame,
        launch_info: &LaunchInfo,
        _proton_path: Option<&str>,
        _launcher_config: &crate::config::LauncherConfig,
        user_config: Option<&crate::models::UserAppConfig>,
    ) -> Result<std::process::Child> {
        let install_dir = match app.install_path.as_ref().map(PathBuf::from) {
            Some(p) if p.exists() => p,
            _ => self.install_root_for_app(app.app_id).await?,
        };

        // Steam VDF stores Windows paths with backslashes; normalize for Linux
        let exe_relative = launch_info.executable.replace('\\', "/");
        let executable = install_dir.join(&exe_relative);
        let mut args = split_args(&launch_info.arguments);

        if let Some(config) = user_config {
            if !config.launch_options.trim().is_empty() {
                args.extend(split_args(&config.launch_options));
            }
        }

        // Standard Steam identity fallback: steam_appid.txt
        let app_id_str = app.app_id.to_string();
        // Resolve working directory:
        // 1. Use VDF-specified workingdir if present (normalized from backslashes)
        // 2. Fall back to executable's parent
        // 3. Fall back to install_dir
        let game_working_dir: PathBuf = launch_info.workingdir
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|wd| install_dir.join(wd.replace('\\', "/")))
            .or_else(|| executable.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| install_dir.clone());

        match launch_info.target {
            LaunchTarget::NativeLinux => {
                let app_id_path = game_working_dir.join("steam_appid.txt");
                std::fs::write(&app_id_path, &app_id_str).unwrap_or_default();

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(metadata) = std::fs::metadata(&executable) {
                        let mut perms = metadata.permissions();
                        perms.set_mode(0o755);
                        let _ = std::fs::set_permissions(&executable, perms);
                    }
                }

                let mut cmd = Command::new(&executable);
                cmd.args(&args);
                cmd.current_dir(&install_dir);

                let bin_dir = executable.parent().unwrap_or_else(|| Path::new("."));
                let existing_ld = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
                let existing_path = std::env::var("PATH").unwrap_or_default();

                cmd.env("LD_LIBRARY_PATH", format!("{}:{}", bin_dir.display(), existing_ld));
                cmd.env("PATH", format!("{}:{}", bin_dir.display(), existing_path));
                cmd.env("SteamAppId", app.app_id.to_string());

                if let Some(config) = user_config {
                    for (key, val) in &config.env_variables {
                        cmd.env(key, val);
                    }
                }

                tracing::info!("Launching game (Native): {:?} with args {:?}", executable, args);
                cmd.spawn().context("failed to spawn native linux game")
            }
            LaunchTarget::WindowsProton => {
                bail!("WindowsProton targets must be launched via the Pipeline and Runner abstraction. Ad-hoc bypass is prohibited.");
            }
        }
    }
}
