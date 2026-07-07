pub mod pipeline;
pub mod stages;
pub mod validators;
pub mod dll_provider_resolver;
pub mod fixups;

#[cfg(test)]
mod verification_tests;

use std::path::{Path, PathBuf};
use anyhow::{Result, Context, anyhow};
use crate::config::{config_dir, LauncherConfig};
use crate::utils::build_runner_command;

pub async fn install_master_steam(config: &LauncherConfig) -> Result<()> {
    let base_dir = config_dir()?;
    let steam_cfg = crate::utils::get_master_steam_config();
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
    let resolved_runner = crate::utils::resolve_runner(&runner_name, &library_root);
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

    let fake_env = crate::utils::setup_fake_steam_trap(&base_dir)?;
    cmd.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &fake_env);
    cmd.env("WINEDLLOVERRIDES", "vstdlib_s=n;tier0_s=n;steamclient=n;steamclient64=n;steam_api=n;steam_api64=n;lsteamclient=");

    for var in ["DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"] {
        if let Ok(value) = std::env::var(var) {
            cmd.env(var, value);
        }
    }

    tracing::info!("Launching Master Steam: {:?}", cmd);

    let _child = cmd.spawn().context("Failed to spawn master steam process")?;

    // Spawned detached: this may be a long-running background Steam or an
    // interactive installer, so we must not block the caller waiting on it.
    Ok(())
}

async fn download_steam_setup(path: &Path) -> Result<()> {
    tracing::info!("Downloading SteamSetup.exe...");
    let url = "https://cdn.akamai.steamstatic.com/client/installer/SteamSetup.exe";
    let response = reqwest::get(url).await?.bytes().await?;
    std::fs::write(path, response)?;
    Ok(())
}

