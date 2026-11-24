//! Host Steam client detection and control.

use anyhow::Result;
use std::path::{Path, PathBuf};

pub fn setup_fake_steam_trap(config_dir: &Path) -> Result<PathBuf> {
    let trap_dir = config_dir.join("fake_env");
    std::fs::create_dir_all(&trap_dir)?;

    let dummy_script = "#!/bin/sh\nexit 0\n";

    let write_executable = |path: &Path| -> Result<()> {
        if path.exists() {
            return Ok(());
        }
        std::fs::write(path, dummy_script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms)?;
        }
        Ok(())
    };

    write_executable(&trap_dir.join("steam"))?;
    write_executable(&trap_dir.join("steam.sh"))?;

    Ok(trap_dir)
}

/// The host's real Steam client install directory, suitable as
/// `STEAM_COMPAT_CLIENT_INSTALL_PATH` so Proton's `lsteamclient` bridge can reach
/// the running Steam client (Steamworks online features, Family-Sharing licences).
/// `None` unless a client is installed.
pub fn host_steam_client_path() -> Option<PathBuf> {
    let root = crate::core::config::detect_steam_path()?;
    #[cfg(not(target_os = "windows"))]
    {
        let path_env = std::env::var_os("PATH");
        let flatpak_dirs = steam_flatpak_app_dirs();
        if !steam_client_installed(&root, path_env.as_deref(), &flatpak_dirs) {
            tracing::debug!(
                "no Steam client found for {}; ignoring host Steam",
                root.display()
            );
            return None;
        }
    }
    Some(root)
}

/// Flatpak app dirs for Valve's Steam.
pub fn steam_flatpak_app_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from(
        "/var/lib/flatpak/app/com.valvesoftware.Steam",
    )];
    if let Ok(home) = crate::core::config::home_dir() {
        dirs.push(home.join(".local/share/flatpak/app/com.valvesoftware.Steam"));
    }
    dirs
}

/// First executable named `name` on `path_env`.
pub fn find_on_path(name: &str, path_env: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let path_env = path_env?;
    std::env::split_paths(path_env)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_file(candidate))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// Whether a real Steam client exists.
pub fn steam_client_installed(
    root: &Path,
    path_env: Option<&std::ffi::OsStr>,
    flatpak_dirs: &[PathBuf],
) -> bool {
    ["ubuntu12_32/steam", "steam.sh", "steam.exe"]
        .iter()
        .any(|rel| root.join(rel).is_file())
        || find_on_path("steam", path_env).is_some()
        || flatpak_dirs.iter().any(|d| d.is_dir())
}

/// Whether the host's Steam client is currently running. Checks `~/.steam/steam.pid`
/// first (the client writes its PID there), then falls back to scanning `/proc` for
/// the `steam` or `steamwebhelper` processes. Detection must be reliable: a
/// false negative would make [`ensure_steam_running`] run `steam` again, which only
/// brings the already-open client to the foreground (the opposite of `-silent`).
#[cfg(target_os = "linux")]
pub fn is_steam_running() -> bool {
    let pid_file = std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".steam/steam.pid"));
    steam_running_in(Path::new("/proc"), pid_file.as_deref())
}

/// Testable core of [`is_steam_running`].
#[cfg(target_os = "linux")]
fn steam_running_in(proc_root: &Path, pid_file: Option<&Path>) -> bool {
    // Pid file names the client.
    if let Some(pid) = pid_file
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|pid| pid.trim().parse::<u32>().ok())
        && proc_entry_is_live(&proc_root.join(pid.to_string()))
    {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str() else { continue };
        if !pid.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) {
            let comm = comm.trim();
            if (comm == "steam" || comm == "steamwebhelper") && proc_entry_is_live(&entry.path()) {
                return true;
            }
        }
    }
    false
}

/// Existing, non-zombie `/proc/<pid>` entry.
#[cfg(target_os = "linux")]
fn proc_entry_is_live(proc_dir: &Path) -> bool {
    if !proc_dir.exists() {
        return false;
    }
    let Ok(status) = std::fs::read_to_string(proc_dir.join("status")) else {
        return true;
    };
    !status
        .lines()
        .find_map(|l| l.strip_prefix("State:"))
        .is_some_and(|state| state.trim_start().starts_with('Z'))
}

#[cfg(not(target_os = "linux"))]
pub fn is_steam_running() -> bool {
    false
}

/// Start the host's Steam client minimised to the tray (`steam -silent`) if it
/// isn't already running, so Steamworks/Family-Sharing has a client to talk to.
/// Fully detached (its own session, no stdio) so it outlives this launch and never
/// blocks it. Best-effort: a failure to start is logged, not fatal.
#[cfg(target_os = "linux")]
pub fn ensure_steam_running() {
    if is_steam_running() {
        tracing::info!("Steam client already running; not starting it");
        return;
    }
    tracing::info!("Steam client not running; starting `steam -silent` (tray)");
    match start_steam_detached(&["-silent"]) {
        Ok(()) => tracing::info!("Launched Steam client (-silent)"),
        Err(e) => tracing::warn!("could not start Steam client: {e:#}"),
    }
}

/// Flatpak app id of Valve's Steam.
pub const STEAM_FLATPAK_APP_ID: &str = "com.valvesoftware.Steam";

/// Program plus leading args starting Steam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamLauncher {
    pub program: PathBuf,
    pub prefix_args: Vec<String>,
}

/// Sandboxed packaging of a Steam root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamRootKind {
    Native,
    Flatpak,
    Snap,
}

/// Classifies a Steam root by path.
pub fn steam_root_kind(root: &Path) -> SteamRootKind {
    let check = |p: &Path| {
        let text = p.to_string_lossy();
        if text.contains(&format!("/.var/app/{STEAM_FLATPAK_APP_ID}/"))
            || text.ends_with(&format!("/.var/app/{STEAM_FLATPAK_APP_ID}"))
        {
            Some(SteamRootKind::Flatpak)
        } else if text.contains("/snap/steam/") {
            Some(SteamRootKind::Snap)
        } else {
            None
        }
    };
    check(root)
        .or_else(|| std::fs::canonicalize(root).ok().and_then(|c| check(&c)))
        .unwrap_or(SteamRootKind::Native)
}

/// Steam launcher; sandboxed roots skip `steam.sh`.
pub fn steam_launcher(
    path_env: Option<&std::ffi::OsStr>,
    root: Option<&Path>,
    flatpak_dirs: &[PathBuf],
) -> Option<SteamLauncher> {
    let kind = root.map_or(SteamRootKind::Native, steam_root_kind);
    let flatpak_run = || {
        let installed = kind == SteamRootKind::Flatpak || flatpak_dirs.iter().any(|d| d.is_dir());
        if !installed {
            return None;
        }
        find_on_path("flatpak", path_env).map(|program| SteamLauncher {
            program,
            prefix_args: vec!["run".into(), STEAM_FLATPAK_APP_ID.into()],
        })
    };
    // Live Flatpak root: use flatpak.
    if kind == SteamRootKind::Flatpak
        && let Some(launcher) = flatpak_run()
    {
        return Some(launcher);
    }
    if let Some(program) = find_on_path("steam", path_env) {
        return Some(SteamLauncher {
            program,
            prefix_args: Vec::new(),
        });
    }
    if kind == SteamRootKind::Native
        && let Some(script) = root
            .map(|r| r.join("steam.sh"))
            .filter(|script| script.is_file())
    {
        return Some(SteamLauncher {
            program: script,
            prefix_args: Vec::new(),
        });
    }
    flatpak_run()
}

/// Spawns Steam detached and reaps it.
#[cfg(target_os = "linux")]
pub fn start_steam_detached(args: &[&str]) -> Result<()> {
    let path_env = std::env::var_os("PATH");
    let root = crate::core::config::detect_steam_path();
    let flatpak_dirs = steam_flatpak_app_dirs();
    let Some(launcher) = steam_launcher(path_env.as_deref(), root.as_deref(), &flatpak_dirs) else {
        tracing::warn!("no usable Steam launcher found");
        anyhow::bail!("no `steam` on PATH, no host steam.sh and no Flatpak Steam");
    };
    let program = launcher.program;
    let mut cmd = std::process::Command::new(&program);
    cmd.args(&launcher.prefix_args)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    crate::core::session_env::current().apply(&mut cmd);
    // Own session; outlives the game.
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("could not start {}: {e}", program.display()))?;
    // Reap it; avoid daemon zombies.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Latest logged-on state in connection-log text.
#[cfg(target_os = "linux")]
fn connection_log_logged_on(text: &str) -> Option<bool> {
    let mut state = None;
    for line in text.lines() {
        if line.contains("[Logged On") {
            state = Some(true);
        } else if line.contains("[Logged Off") {
            state = Some(false);
        }
    }
    state
}

/// Read the final `max_bytes` of a file.
#[cfg(target_os = "linux")]
fn read_tail(path: &Path, max_bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(max_bytes)))
        .ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Host Steam finished logging on.
#[cfg(target_os = "linux")]
pub fn steam_is_logged_on() -> bool {
    let Some(root) = crate::core::config::detect_steam_path() else {
        return false;
    };
    let log = root.join("logs/connection_log.txt");
    let Some(text) = read_tail(&log, 64 * 1024) else {
        return false;
    };
    connection_log_logged_on(&text).unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
pub fn steam_is_logged_on() -> bool {
    true
}

/// Wait until host Steam is logged on.
#[cfg(target_os = "linux")]
pub async fn wait_for_steam_logged_on(timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    let mut announced = false;
    loop {
        if steam_is_logged_on() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        if !announced {
            tracing::info!("waiting for host Steam to reach logged-on before launch");
            announced = true;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

#[cfg(not(target_os = "linux"))]
pub async fn wait_for_steam_logged_on(_timeout: std::time::Duration) -> bool {
    true
}

#[derive(Debug, Clone)]
pub struct MasterSteamConfig {
    pub root_dir: PathBuf,    // e.g. ~/.config/Aurelia/master_steam_prefix
    pub wine_prefix: PathBuf, // e.g. root_dir or root_dir/pfx
    pub layout_kind: String,  // "root" or "pfx"
    pub steam_exe: Option<PathBuf>,
}

pub fn get_master_steam_config() -> MasterSteamConfig {
    let root_dir = crate::core::config::config_dir()
        .unwrap_or_default()
        .join("master_steam_prefix");

    // Layout detection: prefer /pfx if it exists, otherwise check root for drive_c
    let (wine_prefix, layout_kind) = if root_dir.join("pfx/drive_c").exists() {
        (root_dir.join("pfx"), "pfx".to_string())
    } else if root_dir.join("drive_c").exists() {
        (root_dir.clone(), "root".to_string())
    } else {
        // Default for new installs
        (root_dir.join("pfx"), "pfx".to_string())
    };

    let steam_exe = find_steam_exe_in_prefix(&wine_prefix);

    MasterSteamConfig {
        root_dir,
        wine_prefix,
        layout_kind,
        steam_exe,
    }
}

pub fn find_steam_exe_in_prefix(prefix: &Path) -> Option<PathBuf> {
    // Steam's own installer writes `Steam.exe` (capital S). Windows/Wine treat filenames
    // case-insensitively, but the underlying Linux filesystem does not — so a hardcoded
    // lowercase `steam.exe` misses a real install and (before this) made a *successful*
    // `steam-runtime install` report "no steam.exe appeared". Match the leaf
    // case-insensitively within each candidate Steam directory instead.
    let steam_dirs = [
        "drive_c/Program Files (x86)/Steam",
        "drive_c/Program Files/Steam",
    ];

    for rel_dir in steam_dirs {
        let dir = prefix.join(rel_dir);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("steam.exe")
            {
                return Some(entry.path());
            }
        }
    }

    None
}

/// Detects the actual WINEPREFIX layout for the master Steam install.
/// Handles both master_steam_prefix/pfx/drive_c and master_steam_prefix/drive_c layouts.
pub fn resolve_master_wineprefix() -> PathBuf {
    get_master_steam_config().wine_prefix
}

#[cfg(test)]
mod tests;
