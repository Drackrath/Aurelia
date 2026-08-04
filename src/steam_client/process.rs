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
    pub fn stop_game(app_id: u32, force: bool) -> Result<crate::compat::running::RunningGame> {
        let record = crate::compat::running::load(app_id).ok_or_else(|| {
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
        crate::compat::running::clear(app_id);
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
        let mut pids: Vec<i32> = Vec::new();

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
                pids.push(pid);
            }
            // Never short-circuit: sweep every matching process.
            None::<()>
        });

        kill_or_escalate(&pids, force);
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
        let mut pids: Vec<i32> = Vec::new();

        Self::scan_proc_pids(|pid_path, pid_str| {
            let environ = match std::fs::read(pid_path.join("environ")) {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(_) => return None,
            };
            if !environ.contains(&prefix_str) {
                return None;
            }

            if let Ok(pid) = pid_str.parse::<i32>() {
                pids.push(pid);
            }
            // Never short-circuit: sweep every matching process.
            None::<()>
        });

        kill_or_escalate(&pids, force);
    }

    pub fn kill_steam_in_prefix(wineprefix: &Path) {
        #[cfg(unix)]
        {
            let prefix_str = wineprefix.to_string_lossy().to_string();
            let mut pids: Vec<i32> = Vec::new();

            Self::scan_proc_pids(|pid_path, pid_str| {
                let cmdline = match std::fs::read(pid_path.join("cmdline")) {
                    Ok(b) => String::from_utf8_lossy(&b).replace('\0', " "),
                    Err(_) => return None,
                };
                // Kill the Steam client, its CEF helper, and the steamservice.exe
                // Steam respawns to back its client IPC, in this prefix.
                let cmdline_lower = cmdline.to_lowercase();
                if !cmdline_lower.contains("steam.exe")
                    && !cmdline_lower.contains("steamwebhelper.exe")
                    && !cmdline_lower.contains("steamservice.exe")
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
                    pids.push(pid);
                }
                // Never short-circuit: sweep every matching process.
                None::<()>
            });

            // TERM → bounded wait → KILL: a wedged steam.exe used to survive the
            // bare SIGTERM here and outlive repair/stop.
            terminate_pids_with_escalation(&pids);
        }
        #[cfg(not(unix))]
        {
            let _ = wineprefix;
        }
    }

    /// Kill wineserver processes that serve `wineprefix` but belong to a runner
    /// OTHER than the ones rooted at `allowed_runner_roots` (the runner(s) about
    /// to use the prefix, including the background Steam's — so the wineserver of
    /// an intentionally-running same-runner Steam is never touched). A wineserver
    /// left behind by a previous launch under a different wine build keeps the
    /// prefix's session alive with mismatched libraries, and the new launch then
    /// joins the stale session instead of starting its own. Returns how many
    /// stale wineservers were found (always 0 on non-unix).
    pub fn kill_stale_wineservers_in_prefix(
        wineprefix: &Path,
        allowed_runner_roots: &[PathBuf],
    ) -> usize {
        #[cfg(unix)]
        {
            // With no known-good root every wineserver would classify as stale;
            // refuse to sweep rather than kill the legitimate one.
            if allowed_runner_roots.is_empty() {
                return 0;
            }
            let mut stale: Vec<i32> = Vec::new();
            Self::scan_proc_pids(|pid_path, pid_str| {
                let exe = std::fs::read_link(pid_path.join("exe")).ok()?;
                let environ = std::fs::read(pid_path.join("environ")).ok()?;
                if is_stale_cross_runner_wineserver(&exe, &environ, wineprefix, allowed_runner_roots)
                {
                    if let Ok(pid) = pid_str.parse::<i32>() {
                        tracing::warn!(
                            pid,
                            exe = %exe.display(),
                            prefix = %wineprefix.display(),
                            "killing stale cross-runner wineserver holding the prefix"
                        );
                        stale.push(pid);
                    }
                }
                // Never short-circuit: sweep every matching process.
                None::<()>
            });
            if !stale.is_empty() {
                terminate_pids_with_escalation(&stale);
            }
            stale.len()
        }
        #[cfg(not(unix))]
        {
            let _ = (wineprefix, allowed_runner_roots);
            0
        }
    }

    /// Kill Steam helper processes in `wineprefix` that back features the user
    /// disabled. Steam re-spawns `steamwebhelper.exe`/`gameoverlayui.exe` even
    /// when launched with the corresponding disable flags, so the flags alone
    /// don't stick — this enforces them from outside, kill-on-sight (SIGKILL).
    /// Executables are deliberately NOT chmod'd to block respawns: upstream's
    /// chmod-000 approach broke `steam-runtime repair` (maintainer decision
    /// pending).
    ///
    /// Matching is PID-safe: a name match alone is never enough — the process
    /// must also prove it belongs to `wineprefix` via its own environment.
    pub fn enforce_disabled_steam_features_in_prefix(
        wineprefix: &Path,
        no_browser: bool,
        no_friends_ui: bool,
        no_overlay: bool,
        no_chat_ui: bool,
    ) {
        #[cfg(unix)]
        {
            if !(no_browser || no_friends_ui || no_overlay || no_chat_ui) {
                return;
            }
            let prefix_str = wineprefix.to_string_lossy().to_string();
            let mut doomed: Vec<i32> = Vec::new();

            Self::scan_proc_pids(|pid_path, pid_str| {
                let argv = match std::fs::read(pid_path.join("cmdline")) {
                    Ok(b) => cmdline_argv(&b),
                    Err(_) => return None,
                };
                if !disabled_helper_kill_match(&argv, no_browser, no_friends_ui, no_overlay, no_chat_ui)
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
                    doomed.push(pid);
                }
                // Never short-circuit: sweep every matching process.
                None::<()>
            });

            for pid in doomed {
                tracing::info!(pid, "enforcing disabled Steam feature: killing helper process");
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (wineprefix, no_browser, no_friends_ui, no_overlay, no_chat_ui);
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

    /// True while a **game** launched by the in-Wine Steam is running inside
    /// `wineprefix`. The game is identified by `installdir` — the manifest install-dir
    /// name (the last path component of its install path) — which appears in the
    /// process cmdline as `…\steamapps\common\<installdir>\…exe`. The Steam client and
    /// its helpers are excluded so this tracks only the game itself.
    ///
    /// Used to wait on a game the in-Wine Steam started (via `-applaunch`), which
    /// Aurelia does not own directly and so cannot `wait()` on.
    pub fn is_game_running_in_prefix(wineprefix: &Path, installdir: &str) -> bool {
        #[cfg(unix)]
        {
            let prefix_str = wineprefix.to_string_lossy().to_string();
            let needle = installdir.to_lowercase();
            if needle.is_empty() {
                return false;
            }
            Self::scan_proc_pids(|pid_path, _pid_str| {
                let cmdline = std::fs::read(pid_path.join("cmdline")).ok()?;
                let cmdline_str = String::from_utf8_lossy(&cmdline).replace('\0', " ").to_lowercase();
                if !(cmdline_str.contains(&needle) && cmdline_str.contains(".exe")) {
                    return None;
                }
                // Exclude the Steam client and its CEF helper processes.
                if cmdline_str.contains("steam.exe") || cmdline_str.contains("steamwebhelper") {
                    return None;
                }
                let environ = std::fs::read(pid_path.join("environ")).ok()?;
                String::from_utf8_lossy(&environ).contains(&prefix_str).then_some(true)
            })
            .unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            let _ = (wineprefix, installdir);
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
        user_config: Option<&crate::core::models::UserAppConfig>,
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
        launcher_config: &crate::core::config::LauncherConfig,
        user_config: Option<&crate::core::models::UserAppConfig>,
        force_native_engine: bool,
        force_umu: bool,
        launch_script_override: Option<PathBuf>,
        disable_launch_script: bool,
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
        ctx.force_umu = force_umu;
        ctx.launch_script_override = launch_script_override;
        ctx.disable_launch_script = disable_launch_script;
        ctx.steam_enabled = steam_enabled;

        if let Ok(config_dir) = crate::core::config::config_dir() {
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
        _launcher_config: &crate::core::config::LauncherConfig,
        user_config: Option<&crate::core::models::UserAppConfig>,
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

/// Split a raw `/proc/<pid>/cmdline` buffer (NUL-separated argv) into strings,
/// dropping empty entries (the buffer is NUL-terminated).
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn cmdline_argv(raw: &[u8]) -> Vec<String> {
    raw.split(|&b| b == 0)
        .filter(|e| !e.is_empty())
        .map(|e| String::from_utf8_lossy(e).into_owned())
        .collect()
}

/// True when a scanned process is a wineserver that serves `target_prefix` but
/// whose executable lives outside every runner root in `allowed_runner_roots` —
/// i.e. a stale server left behind by a launch under a different wine build.
///
/// The WINEPREFIX comparison is an exact env-entry match (never a substring),
/// with trailing slashes normalized, so `/pfx` never matches `/pfx2`. Pure over
/// its inputs so the classification is unit-testable with synthetic records.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn is_stale_cross_runner_wineserver(
    exe_path: &Path,
    environ: &[u8],
    target_prefix: &Path,
    allowed_runner_roots: &[PathBuf],
) -> bool {
    // /proc/<pid>/exe of an updated/removed binary keeps the old path with a
    // " (deleted)" suffix — still a wineserver.
    let is_wineserver = exe_path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("wineserver"));
    if !is_wineserver {
        return false;
    }

    let target = target_prefix.to_string_lossy();
    let target = target.trim_end_matches('/');
    let prefix_matches = environ.split(|&b| b == 0).any(|entry| {
        entry
            .strip_prefix(&b"WINEPREFIX="[..])
            .is_some_and(|v| String::from_utf8_lossy(v).trim_end_matches('/') == target)
    });
    if !prefix_matches {
        return false;
    }

    !allowed_runner_roots.iter().any(|root| exe_path.starts_with(root))
}

/// Whether a process with this argv backs a disabled Steam feature and should be
/// killed. `steamwebhelper.exe` hosts ALL of Steam's web UI (browser, friends,
/// chat) with no per-surface process to target, so any of those three flags
/// condemns it; it is matched by cmdline substring because helper subprocesses
/// carry the exe at varying argv positions. `gameoverlayui.exe` is matched on
/// argv[0]'s exact basename only — a substring match could hit a game process
/// merely passing overlay-related arguments.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn disabled_helper_kill_match(
    argv: &[String],
    no_browser: bool,
    no_friends_ui: bool,
    no_overlay: bool,
    no_chat_ui: bool,
) -> bool {
    if (no_browser || no_friends_ui || no_chat_ui)
        && argv.iter().any(|a| a.to_lowercase().contains("steamwebhelper.exe"))
    {
        return true;
    }
    if no_overlay {
        if let Some(first) = argv.first() {
            let base = first.replace('\\', "/");
            let base = base.rsplit('/').next().unwrap_or("");
            if base.eq_ignore_ascii_case("gameoverlayui.exe") {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environ(entries: &[&str]) -> Vec<u8> {
        let mut buf = Vec::new();
        for e in entries {
            buf.extend_from_slice(e.as_bytes());
            buf.push(0);
        }
        buf
    }

    #[test]
    fn stale_wineserver_requires_wineserver_binary() {
        let env = environ(&["WINEPREFIX=/home/u/pfx"]);
        assert!(!is_stale_cross_runner_wineserver(
            Path::new("/opt/other-wine/bin/wine64"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg")],
        ));
    }

    #[test]
    fn stale_wineserver_requires_exact_prefix_match() {
        // Substring / sibling prefixes must never match.
        let env = environ(&["WINEPREFIX=/home/u/pfx2"]);
        assert!(!is_stale_cross_runner_wineserver(
            Path::new("/opt/other-wine/bin/wineserver"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg")],
        ));
        // No WINEPREFIX at all (default ~/.wine) is not a match either.
        let env = environ(&["HOME=/home/u"]);
        assert!(!is_stale_cross_runner_wineserver(
            Path::new("/opt/other-wine/bin/wineserver"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg")],
        ));
    }

    #[test]
    fn stale_wineserver_tolerates_trailing_slash() {
        let env = environ(&["WINEPREFIX=/home/u/pfx/"]);
        assert!(is_stale_cross_runner_wineserver(
            Path::new("/opt/other-wine/bin/wineserver"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg")],
        ));
    }

    #[test]
    fn same_runner_wineserver_is_not_stale() {
        let env = environ(&["WINEPREFIX=/home/u/pfx"]);
        // Belongs to the game runner root — allowed.
        assert!(!is_stale_cross_runner_wineserver(
            Path::new("/opt/wine-tkg/bin/wineserver"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg")],
        ));
        // Belongs to the background-Steam runner root (second allowed root) —
        // the intentionally-running Steam's wineserver must survive the sweep.
        assert!(!is_stale_cross_runner_wineserver(
            Path::new("/opt/proton/files/bin/wineserver"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg"), PathBuf::from("/opt/proton/files")],
        ));
    }

    #[test]
    fn cross_runner_wineserver_is_stale() {
        let env = environ(&["WINEPREFIX=/home/u/pfx", "DISPLAY=:0"]);
        assert!(is_stale_cross_runner_wineserver(
            Path::new("/opt/other-wine/bin/wineserver"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg")],
        ));
        // A deleted (updated) binary keeps a " (deleted)" suffix — still stale.
        assert!(is_stale_cross_runner_wineserver(
            Path::new("/opt/other-wine/bin/wineserver (deleted)"),
            &env,
            Path::new("/home/u/pfx"),
            &[PathBuf::from("/opt/wine-tkg")],
        ));
    }

    #[test]
    fn webhelper_matched_by_cmdline_substring() {
        let argv = vec![
            "C:\\Program Files (x86)\\Steam\\bin\\cef\\cef.win7x64\\steamwebhelper.exe".to_string(),
            "--type=renderer".to_string(),
        ];
        assert!(disabled_helper_kill_match(&argv, true, false, false, false));
        assert!(disabled_helper_kill_match(&argv, false, true, false, false));
        assert!(disabled_helper_kill_match(&argv, false, false, false, true));
        // Overlay-only disable does not condemn the webhelper.
        assert!(!disabled_helper_kill_match(&argv, false, false, true, false));
        // Nothing disabled — nothing killed.
        assert!(!disabled_helper_kill_match(&argv, false, false, false, false));
    }

    #[test]
    fn overlay_matched_by_exact_argv0_basename_only() {
        let overlay = vec!["C:\\Steam\\gameoverlayui.exe".to_string(), "-pid".to_string()];
        assert!(disabled_helper_kill_match(&overlay, false, false, true, false));
        assert!(!disabled_helper_kill_match(&overlay, true, true, false, true));

        // A game merely mentioning the overlay in its args is NOT a match.
        let game = vec![
            "C:\\game\\game.exe".to_string(),
            "-watchdog=gameoverlayui.exe".to_string(),
        ];
        assert!(!disabled_helper_kill_match(&game, false, false, true, false));

        // steam.exe itself is never matched by this enforcement.
        let steam = vec!["C:\\Steam\\steam.exe".to_string(), "-silent".to_string()];
        assert!(!disabled_helper_kill_match(&steam, true, true, true, true));
    }

    #[test]
    fn cmdline_argv_splits_on_nul() {
        let raw = b"C:\\Steam\\steam.exe\0-silent\0\0";
        assert_eq!(cmdline_argv(raw), vec!["C:\\Steam\\steam.exe", "-silent"]);
    }
}
