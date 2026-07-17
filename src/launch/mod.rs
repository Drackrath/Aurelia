pub mod pipeline;
pub mod stages;
pub mod validators;
pub mod dll_provider_resolver;
pub mod fixups;
pub mod launch_script;

#[cfg(test)]
mod verification_tests;

use std::path::{Path, PathBuf};
use std::process::Command;
use anyhow::{Result, Context, anyhow};
use crate::core::config::{config_dir, LauncherConfig};
use crate::steam_client::SteamClient;
use crate::core::utils::MasterSteamConfig;

/// Ensure Steam is installed into the master Windows prefix, then start it.
///
/// Two distinct phases with very different process semantics:
///
/// 1. If `steam.exe` is absent, run `SteamSetup.exe` **synchronously** under a bare
///    wine and verify it actually produced `steam.exe`. The installer is a bounded
///    job, so we wait on it and surface its exit code.
/// 2. Launch `steam.exe` **detached** — that is the long-running background Steam
///    client and must not block the caller.
pub async fn install_master_steam(config: &LauncherConfig) -> Result<()> {
    let base_dir = config_dir()?;
    let steam_cfg = crate::core::utils::get_master_steam_config();

    // Resolve the runner FIRST: a misconfigured runner must fail before we spend a
    // download on an installer we have no way to execute.
    let runner_name = config.steam_runtime_runner.to_string_lossy();
    let library_root = PathBuf::from(&config.steam_library_path);
    let wine = crate::core::utils::resolve_steam_runtime_wine(&runner_name, &library_root)?;

    tracing::info!("Unified Master Steam resolution:");
    tracing::info!("  - Root Dir: {}", steam_cfg.root_dir.display());
    tracing::info!("  - Wine Prefix: {}", steam_cfg.wine_prefix.display());
    tracing::info!("  - Layout Kind: {}", steam_cfg.layout_kind);
    tracing::info!("  - Wine Binary: {}", wine.display());

    let steam_exe = match steam_cfg.steam_exe.clone() {
        Some(exe) => {
            tracing::info!("  - Steam Exe: {} (already installed)", exe.display());
            exe
        }
        None => {
            tracing::info!("  - Steam Exe: NOT FOUND (running installer)");
            run_steam_installer(&wine, &steam_cfg, &base_dir).await?
        }
    };

    launch_master_steam(&wine, &steam_exe, &steam_cfg, &base_dir)
}

/// Download (if needed) and run `SteamSetup.exe` to completion under `wine`.
/// Returns the path to the installed `steam.exe`.
async fn run_steam_installer(
    wine: &Path,
    steam_cfg: &MasterSteamConfig,
    base_dir: &Path,
) -> Result<PathBuf> {
    let runtimes_dir = base_dir.join("runtimes");
    std::fs::create_dir_all(&runtimes_dir)?;
    let setup_exe = runtimes_dir.join("SteamSetup.exe");
    ensure_steam_setup(&setup_exe).await?;

    // Create the WINEPREFIX ourselves before invoking the installer. On a fresh
    // install the `pfx` layout points WINEPREFIX at `root_dir/pfx`, whose parent
    // does not exist yet; SteamSetup.exe then ran against a missing prefix and users
    // had to `mkdir -p .../master_steam_prefix/pfx` by hand first (issue #2).
    std::fs::create_dir_all(&steam_cfg.wine_prefix).with_context(|| {
        format!(
            "Failed to create master Steam prefix at {}",
            steam_cfg.wine_prefix.display()
        )
    })?;

    let mut cmd = Command::new(wine);
    cmd.arg(&setup_exe);
    // `/S` is the NSIS silent-install switch. Without it SteamSetup.exe opens its
    // interactive wizard and waits for a human, so an `install` on a headless or
    // unattended machine simply never completes.
    cmd.arg("/S");
    apply_master_steam_env(&mut cmd, steam_cfg, base_dir)?;
    apply_install_diagnostics(&mut cmd, base_dir);

    tracing::info!("Running Steam installer: {:?}", cmd);

    // Wait for the installer: it is a bounded job, unlike background Steam. The old
    // code spawned it detached and dropped the child, so a wine that died on startup
    // still reported "install started" and left the caller with no diagnostics.
    let status = tokio::process::Command::from(cmd)
        .status()
        .await
        .with_context(|| format!("Failed to run Steam installer under {}", wine.display()))?;

    if !status.success() {
        return Err(anyhow!(
            "SteamSetup.exe failed under {} (exit status: {status}). \
             Re-run with AURELIA_DIAGNOSE_INSTALL=1 to capture a wine debug log.",
            wine.display()
        ));
    }

    // Re-probe rather than trusting the exit code: NSIS happily returns 0 when it
    // silently declines to install anything.
    crate::core::utils::get_master_steam_config()
        .steam_exe
        .ok_or_else(|| {
            anyhow!(
                "SteamSetup.exe exited successfully but no steam.exe appeared under {}. \
                 Re-run with AURELIA_DIAGNOSE_INSTALL=1 to capture a wine debug log.",
                steam_cfg.wine_prefix.display()
            )
        })
}

/// Start the background Steam client detached. It is long-running by design, so the
/// caller must not wait on it.
fn launch_master_steam(
    wine: &Path,
    steam_exe: &Path,
    steam_cfg: &MasterSteamConfig,
    base_dir: &Path,
) -> Result<()> {
    let mut cmd = Command::new(wine);
    cmd.arg(steam_exe);
    // Steam *client* flags — these mean nothing to the installer and were previously
    // passed to it as well.
    cmd.arg("-tcp");
    cmd.arg("-cef-disable-gpu-compositing");
    apply_master_steam_env(&mut cmd, steam_cfg, base_dir)?;
    apply_install_diagnostics(&mut cmd, base_dir);

    tracing::info!("Launching Master Steam: {:?}", cmd);
    let _child = cmd.spawn().context("Failed to spawn master steam process")?;
    Ok(())
}

/// Environment shared by the installer and the background Steam client.
fn apply_master_steam_env(
    cmd: &mut Command,
    steam_cfg: &MasterSteamConfig,
    base_dir: &Path,
) -> Result<()> {
    cmd.env("WINEPREFIX", &steam_cfg.wine_prefix);
    cmd.env("STEAM_COMPAT_DATA_PATH", &steam_cfg.root_dir);
    cmd.env("WINEPATH", "C:\\Program Files (x86)\\Steam");

    let fake_env = crate::core::utils::setup_fake_steam_trap(base_dir)?;
    cmd.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &fake_env);
    cmd.env("WINEDLLOVERRIDES", "vstdlib_s=n;tier0_s=n;steamclient=n;steamclient64=n;steam_api=n;steam_api64=n;lsteamclient=");

    for var in ["DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"] {
        if let Ok(value) = std::env::var(var) {
            cmd.env(var, value);
        }
    }
    Ok(())
}

/// Opt-in Steam-runtime install diagnostics.
///
/// When `AURELIA_DIAGNOSE_INSTALL=1` the install/repair flow runs with verbose
/// WINEDEBUG channels (setupapi/file/module) that surface the file-copy and
/// DLL-registration failures typical of a broken Steam install, and its stdout/stderr
/// are captured to a timestamped log file under `config_dir()/logs`, the same root the
/// launch pipeline uses. This path is ISOLATED to master-Steam install/repair and never
/// touches normal game launches. With the var unset, behavior is unchanged.
fn apply_install_diagnostics(cmd: &mut Command, base_dir: &Path) {
    if std::env::var("AURELIA_DIAGNOSE_INSTALL").as_deref() != Ok("1") {
        return;
    }
    let logs_dir = base_dir.join("logs");
    if let Err(e) = std::fs::create_dir_all(&logs_dir) {
        tracing::warn!("AURELIA_DIAGNOSE_INSTALL set but could not create log dir {}: {}", logs_dir.display(), e);
        return;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let log_path = logs_dir.join(format!("steam_runtime_install_{stamp}.log"));
    match std::fs::File::create(&log_path) {
        Ok(file) => {
            cmd.env("WINEDEBUG", "+setupapi,+file,+module");
            if let Ok(err_file) = file.try_clone() {
                cmd.stderr(std::process::Stdio::from(err_file));
            }
            cmd.stdout(std::process::Stdio::from(file));
            tracing::info!(
                "AURELIA_DIAGNOSE_INSTALL=1: capturing Steam-runtime install diagnostics to {}",
                log_path.display()
            );
        }
        Err(e) => tracing::warn!(
            "AURELIA_DIAGNOSE_INSTALL set but could not create diagnostic log {}: {}",
            log_path.display(), e
        ),
    }
}

/// Repair the master Windows-Steam prefix: stop anything holding it, snapshot the
/// current prefix (retaining a single `.bak`), then re-run the installer into a
/// fresh prefix.
///
/// Like [`install_master_steam`], this needs a configured `steam_runtime_runner`
/// to drive the installer under a bare wine. The runner is validated up front so
/// the destructive backup step never runs when the reinstall would fail anyway.
pub async fn repair_master_steam(config: &LauncherConfig) -> Result<()> {
    if config.steam_runtime_runner.as_os_str().is_empty() {
        return Err(anyhow!(
            "{}",
            crate::core::utils::steam_runtime_runner_unset_msg("repairing")
        ));
    }

    let steam_cfg = crate::core::utils::get_master_steam_config();

    // 1. Kill any master-Steam / game processes still holding the prefix so the
    //    directory can be moved safely. Reuse the existing prefix-scoped killers
    //    rather than inventing a new mechanism. `kill_steam_in_prefix` is
    //    cross-platform (a no-op on Windows); the broader wine sweep is unix-only.
    tracing::info!(
        "Repair: stopping any processes holding the master prefix {}",
        steam_cfg.wine_prefix.display()
    );
    SteamClient::kill_steam_in_prefix(&steam_cfg.wine_prefix);
    #[cfg(unix)]
    SteamClient::kill_wine_processes_in_prefix(&steam_cfg.wine_prefix, true);

    // 2. Snapshot the current prefix, retaining only ONE backup. Only if present.
    if steam_cfg.wine_prefix.exists() {
        let mut bak = steam_cfg.wine_prefix.clone().into_os_string();
        bak.push(".bak");
        let bak = PathBuf::from(bak);
        if bak.exists() {
            tracing::info!("Repair: removing previous backup {}", bak.display());
            std::fs::remove_dir_all(&bak)
                .with_context(|| format!("failed removing previous backup {}", bak.display()))?;
        }
        tracing::info!(
            "Repair: backing up {} -> {}",
            steam_cfg.wine_prefix.display(),
            bak.display()
        );
        std::fs::rename(&steam_cfg.wine_prefix, &bak)
            .with_context(|| format!("failed backing up master prefix to {}", bak.display()))?;
    } else {
        tracing::info!(
            "Repair: no existing master prefix at {} — nothing to back up",
            steam_cfg.wine_prefix.display()
        );
    }

    // 3. Re-run the installer into the now-clean prefix.
    install_master_steam(config).await
}

/// (Re-)start the master Steam client **interactively** so the user can sign in —
/// e.g. after the in-prefix Steam session expired. Unlike a game launch (which starts
/// Steam `-silent`), this brings up the client UI. Any Steam already running in the
/// master prefix is stopped first so a real login window appears instead of the
/// request re-attaching to a silent background instance.
///
/// The in-Wine Steam client keeps its **own** login state in the master prefix,
/// independent of `aurelia login`; this is how you refresh it without reinstalling.
pub async fn relogin_master_steam(config: &LauncherConfig) -> Result<()> {
    let base_dir = config_dir()?;
    let steam_cfg = crate::core::utils::get_master_steam_config();

    let steam_exe = steam_cfg.steam_exe.clone().ok_or_else(|| {
        anyhow!(
            "the Windows Steam runtime is not installed yet (no steam.exe under {}). \
             Run `aurelia steam-runtime install` first.",
            steam_cfg.wine_prefix.display()
        )
    })?;

    let runner_name = config.steam_runtime_runner.to_string_lossy();
    let library_root = PathBuf::from(&config.steam_library_path);
    let wine = crate::core::utils::resolve_steam_runtime_wine(&runner_name, &library_root)?;

    // Stop any running (typically `-silent`) in-prefix Steam so the login UI opens.
    SteamClient::kill_steam_in_prefix(&steam_cfg.wine_prefix);
    #[cfg(unix)]
    SteamClient::kill_wine_processes_in_prefix(&steam_cfg.wine_prefix, true);

    launch_master_steam(&wine, &steam_exe, &steam_cfg, &base_dir)
}

/// True when `path` looks like a real Windows executable.
///
/// PE binaries open with the `MZ` DOS header. The previous code only checked
/// `exists()`, so a CDN error page or a download interrupted midway was cached as
/// `SteamSetup.exe` and reused forever — every later install would "succeed" at the
/// download step and then hand wine a file it could not execute.
pub fn is_valid_setup_exe(path: &Path) -> bool {
    use std::io::Read;
    let mut header = [0u8; 2];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .is_ok()
        && &header == b"MZ"
}

/// Download `SteamSetup.exe` unless a valid one is already cached.
async fn ensure_steam_setup(path: &Path) -> Result<()> {
    if is_valid_setup_exe(path) {
        tracing::info!("Using cached SteamSetup.exe at {}", path.display());
        return Ok(());
    }
    if path.exists() {
        tracing::warn!(
            "Cached {} is not a valid Windows executable — re-downloading",
            path.display()
        );
    }
    download_steam_setup(path).await
}

async fn download_steam_setup(path: &Path) -> Result<()> {
    tracing::info!("Downloading SteamSetup.exe...");
    let url = "https://cdn.akamai.steamstatic.com/client/installer/SteamSetup.exe";
    let bytes = reqwest::get(url)
        .await
        .context("Failed to reach the Steam CDN to download SteamSetup.exe")?
        .error_for_status()
        .context("Steam CDN rejected the SteamSetup.exe download")?
        .bytes()
        .await
        .context("Failed to read the SteamSetup.exe response body")?;

    if bytes.len() < 2 || &bytes[..2] != b"MZ" {
        return Err(anyhow!(
            "Downloaded SteamSetup.exe is not a Windows executable ({} bytes from {url})",
            bytes.len()
        ));
    }

    // Write to a temp file and rename, so an interrupted write can never leave a
    // truncated SteamSetup.exe behind.
    let tmp = path.with_extension("exe.part");
    std::fs::write(&tmp, &bytes)
        .with_context(|| format!("failed writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("failed moving {} into place at {}", tmp.display(), path.display()))?;
    Ok(())
}

