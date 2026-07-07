pub mod pipeline;
pub mod stages;
pub mod validators;
pub mod dll_provider_resolver;
pub mod fixups;

#[cfg(test)]
mod verification_tests;

use std::path::{Path, PathBuf};
use anyhow::{Result, Context, anyhow};
use crate::core::config::{config_dir, LauncherConfig};
use crate::steam_client::SteamClient;
use crate::core::utils::build_runner_command;

pub async fn install_master_steam(config: &LauncherConfig) -> Result<()> {
    let base_dir = config_dir()?;
    let steam_cfg = crate::core::utils::get_master_steam_config();
    let runtimes_dir = base_dir.join("runtimes");
    std::fs::create_dir_all(&runtimes_dir)?;

    let setup_exe = runtimes_dir.join("SteamSetup.exe");
    if !setup_exe.exists() {
        download_steam_setup(&setup_exe).await?;
    }

    let runner_name = config.steam_runtime_runner.to_string_lossy();
    if runner_name.is_empty() {
        return Err(anyhow!("No Steam Runtime Runner selected in Global Settings"));
    }

    let library_root = PathBuf::from(&config.steam_library_path);
    let resolved_runner = crate::core::utils::resolve_runner(&runner_name, &library_root);
    let mut cmd = build_runner_command(&resolved_runner)?;

    tracing::info!("Unified Master Steam resolution:");
    tracing::info!("  - Root Dir: {}", steam_cfg.root_dir.display());
    tracing::info!("  - Wine Prefix: {}", steam_cfg.wine_prefix.display());
    tracing::info!("  - Layout Kind: {}", steam_cfg.layout_kind);
    if let Some(ref exe) = steam_cfg.steam_exe {
        tracing::info!("  - Steam Exe: {}", exe.display());
        cmd.arg(exe);
    } else {
        tracing::info!("  - Steam Exe: NOT FOUND (running installer)");
        cmd.arg(setup_exe);
    }

    // Arguments
    cmd.arg("-tcp");
    cmd.arg("-cef-disable-gpu-compositing");

    // Environment Variables
    cmd.env("WINEPREFIX", &steam_cfg.wine_prefix);
    cmd.env("STEAM_COMPAT_DATA_PATH", &steam_cfg.root_dir);
    cmd.env("WINEPATH", "C:\\Program Files (x86)\\Steam");

    let fake_env = crate::core::utils::setup_fake_steam_trap(&base_dir)?;
    cmd.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &fake_env);
    cmd.env("WINEDLLOVERRIDES", "vstdlib_s=n;tier0_s=n;steamclient=n;steamclient64=n;steam_api=n;steam_api64=n;lsteamclient=");

    for var in ["DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"] {
        if let Ok(value) = std::env::var(var) {
            cmd.env(var, value);
        }
    }

    // --- P7: opt-in Steam-runtime install diagnostics -----------------------
    // When AURELIA_DIAGNOSE_INSTALL=1 the install/repair flow runs with verbose
    // WINEDEBUG channels (setupapi/file/module) that surface the file-copy and
    // DLL-registration failures typical of a broken Steam install, and its
    // stdout/stderr are captured to a timestamped log file under the app log
    // directory (reusing config_dir()/logs, the same root the launch pipeline
    // uses). This path is ISOLATED to master-Steam install/repair and never
    // touches normal game launches. With the var unset, behavior is unchanged.
    if std::env::var("AURELIA_DIAGNOSE_INSTALL").as_deref() == Ok("1") {
        let logs_dir = base_dir.join("logs");
        if let Err(e) = std::fs::create_dir_all(&logs_dir) {
            tracing::warn!("AURELIA_DIAGNOSE_INSTALL set but could not create log dir {}: {}", logs_dir.display(), e);
        } else {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let log_path = logs_dir.join(format!("steam_runtime_install_{stamp}.log"));
            match std::fs::File::create(&log_path) {
                Ok(file) => {
                    // Verbose channels useful for setupapi / file-copy / module
                    // load failures during the Steam install.
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
    }
    // --- end P7 diagnostics --------------------------------------------------

    tracing::info!("Launching Master Steam: {:?}", cmd);

    let _child = cmd.spawn().context("Failed to spawn master steam process")?;

    // Spawned detached: this may be a long-running background Steam or an
    // interactive installer, so we must not block the caller waiting on it.
    Ok(())
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
            "No Steam Runtime Runner selected — set `steam_runtime_runner` in Global Settings before repairing"
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

async fn download_steam_setup(path: &Path) -> Result<()> {
    tracing::info!("Downloading SteamSetup.exe...");
    let url = "https://cdn.akamai.steamstatic.com/client/installer/SteamSetup.exe";
    let response = reqwest::get(url).await?.bytes().await?;
    std::fs::write(path, response)?;
    Ok(())
}

