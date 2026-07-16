//! `runtimes` command handlers.

use crate::commands::common::*;

use std::sync::{Arc, RwLock};
use anyhow::{bail, Context, Result};
use aurelia::core::config::load_launcher_config;
use aurelia::core::models::{DepotPlatform, DownloadState};

/// `proton list`: show installable runtimes and what's installed.
pub(crate) async fn cmd_proton_list(installed_only: bool, json: bool) -> Result<()> {
    let cfg = load_launcher_config().await?;
    let installed =
        aurelia::compat::proton::list_installed(std::path::Path::new(&cfg.steam_library_path));
    let default = cfg.proton_version;
    let is_installed = |name: &str| installed.iter().any(|i| i.name.eq_ignore_ascii_case(name));

    if installed_only {
        if json {
            print_json(&serde_json::json!({ "default": default, "installed": installed }));
            return Ok(());
        }
        if installed.is_empty() {
            cli_println!("No Proton/Wine runtimes installed.");
            return Ok(());
        }
        cli_println!("Installed runtimes:");
        for i in &installed {
            let star = if i.name == default { "  * default" } else { "" };
            cli_println!("  {:<28} ({}){star}", i.name, i.location);
        }
        return Ok(());
    }

    let available = aurelia::compat::proton::list_available().await.unwrap_or_default();
    if json {
        print_json(&serde_json::json!({
            "default": default,
            "installed": installed,
            "available": available,
        }));
        return Ok(());
    }

    cli_println!("{:<10}  {:<28}  {:>10}  STATUS", "SOURCE", "NAME", "SIZE");
    for pkg in &available {
        let size = if pkg.size > 0 { human_bytes(pkg.size) } else { "-".to_string() };
        let mut status = String::new();
        if is_installed(&pkg.name) {
            status.push_str("installed");
        }
        if pkg.name == default {
            status.push_str(" *default");
        }
        cli_println!("{:<10}  {:<28}  {:>10}  {}", pkg.label, pkg.name, size, status.trim());
    }
    // Surface any installed runtime not present in the available list (e.g. an old GE
    // build no longer in recent releases) so the user still sees everything on disk.
    for i in &installed {
        if !available.iter().any(|p| p.name.eq_ignore_ascii_case(&i.name)) {
            let star = if i.name == default { " *default" } else { "" };
            cli_println!("{:<10}  {:<28}  {:>10}  installed{star}", "(local)", i.name, "-");
        }
    }
    cli_println!("\nInstall with `aurelia proton install <NAME>`.");
    Ok(())
}

/// `proton install`: download/install a runtime and make it the global default.
pub(crate) async fn cmd_proton_install(version: String, json: bool) -> Result<()> {
    let pkg = aurelia::compat::proton::resolve_package(&version).await?;

    match &pkg.source {
        aurelia::compat::proton::ProtonSource::Valve { app_id } => {
            let app_id = *app_id;
            if !json {
                cli_println!("Installing {} via Steam (app {app_id}) ...", pkg.name);
            }
            let client = authed_client().await?;
            let state = Arc::new(RwLock::new(DownloadState::default()));
            let rx = client
                .install_game(app_id, DepotPlatform::Linux, None, None, None, None, None, state)
                .await
                .with_context(|| format!("failed to start installing {}", pkg.name))?;
            drive_progress(rx, json).await?;
        }
        aurelia::compat::proton::ProtonSource::Github { .. } => {
            if !json {
                cli_println!("Downloading {} ({}) ...", pkg.name, pkg.label);
            }
            let mut last_pct: i64 = -1;
            let mut on_progress = |done: u64, total: u64| {
                if json || total == 0 {
                    return;
                }
                let pct = (done.saturating_mul(100) / total) as i64;
                if pct != last_pct {
                    last_pct = pct;
                    cli_print!(
                        "\r  {pct:>3}%  ({} / {})        ",
                        human_bytes(done),
                        human_bytes(total)
                    );
                }
            };
            let path = aurelia::compat::proton::install_github_package(&pkg, &mut on_progress).await?;
            if !json {
                cli_println!("\n  Extracted to {}", path.display());
            }
        }
    }

    // The freshly installed runtime becomes the global default ("last downloaded").
    let mut cfg = load_launcher_config().await?;
    cfg.proton_version = pkg.name.clone();
    cfg.save().await.context("failed saving the default Proton version")?;

    if json {
        print_json(&serde_json::json!({
            "name": pkg.name,
            "status": "installed",
            "default": true,
        }));
    } else {
        cli_println!("Installed {} and set it as the global default.", pkg.name);
    }
    Ok(())
}

/// `proton uninstall`: delete an installed custom (GE) runtime.
pub(crate) async fn cmd_proton_uninstall(version: String, json: bool) -> Result<()> {
    aurelia::compat::proton::remove(&version)
        .with_context(|| format!("failed to uninstall {version}"))?;

    // If it was the global default, the default now points at something gone; warn.
    let cfg = load_launcher_config().await?;
    let was_default = cfg.proton_version.eq_ignore_ascii_case(&version);

    if json {
        print_json(&serde_json::json!({
            "name": version,
            "status": "uninstalled",
            "was_default": was_default,
        }));
    } else {
        cli_println!("Uninstalled {version}.");
        if was_default {
            cli_eprintln!(
                "Note: {version} was the global default — set a new one with \
                 `aurelia proton default <NAME>`."
            );
        }
    }
    Ok(())
}

/// `proton default`: set the global default Proton/Wine version.
pub(crate) async fn cmd_proton_default(version: String, json: bool) -> Result<()> {
    let mut cfg = load_launcher_config().await?;
    let installed =
        aurelia::compat::proton::list_installed(std::path::Path::new(&cfg.steam_library_path));
    let present = installed.iter().any(|i| i.name.eq_ignore_ascii_case(&version));

    cfg.proton_version = version.clone();
    cfg.save().await.context("failed saving the default Proton version")?;

    if json {
        print_json(&serde_json::json!({
            "default": version,
            "installed_present": present,
        }));
    } else {
        cli_println!("Global default Proton set to {version}.");
        if !present {
            cli_eprintln!(
                "Note: '{version}' is not installed — run `aurelia proton install {version}`."
            );
        }
    }
    Ok(())
}

/// `steam-runtime install`: install Steam into the master Windows prefix.
pub(crate) async fn cmd_steam_runtime_install(json: bool) -> Result<()> {
    let config = load_launcher_config().await?;
    // Pre-check here (before install_master_steam downloads SteamSetup.exe) so an
    // unconfigured runner fails fast with an actionable message and no wasted work.
    if config.steam_runtime_runner.as_os_str().is_empty() {
        bail!("{}", aurelia::core::utils::steam_runtime_runner_unset_msg("installing"));
    }
    aurelia::launch::install_master_steam(&config).await?;
    // install_master_steam now waits for SteamSetup.exe and verifies steam.exe exists
    // before returning, so reaching this point means the install genuinely landed.
    let steam_cfg = aurelia::core::utils::get_master_steam_config();
    if json {
        print_json(&serde_json::json!({
            "status": "installed",
            "steam_exe": steam_cfg.steam_exe,
        }));
    } else {
        cli_println!("Master Steam runtime installed; Steam client started.");
        if let Some(exe) = &steam_cfg.steam_exe {
            cli_println!("steam.exe path    : {}", exe.display());
        }
    }
    Ok(())
}

/// `steam-runtime repair`: back up the master prefix and reinstall.
pub(crate) async fn cmd_steam_runtime_repair(json: bool) -> Result<()> {
    let config = load_launcher_config().await?;
    if config.steam_runtime_runner.as_os_str().is_empty() {
        bail!("{}", aurelia::core::utils::steam_runtime_runner_unset_msg("repairing"));
    }
    aurelia::launch::repair_master_steam(&config).await?;
    let steam_cfg = aurelia::core::utils::get_master_steam_config();
    if json {
        print_json(&serde_json::json!({
            "status": "repaired",
            "steam_exe": steam_cfg.steam_exe,
        }));
    } else {
        cli_println!("Master Steam runtime repaired (backed up old prefix, reinstalled).");
    }
    Ok(())
}

/// `steam-runtime status`: report the resolved master prefix and configuration.
pub(crate) async fn cmd_steam_runtime_status(json: bool) -> Result<()> {
    let config = load_launcher_config().await?;
    let steam_cfg = aurelia::core::utils::get_master_steam_config();
    let steam_exe_present = steam_cfg.steam_exe.is_some();
    let runner = config.steam_runtime_runner.to_string_lossy().to_string();
    let runner_configured = !runner.is_empty();

    if json {
        print_json(&serde_json::json!({
            "root_dir": steam_cfg.root_dir,
            "wine_prefix": steam_cfg.wine_prefix,
            "layout_kind": steam_cfg.layout_kind,
            "steam_exe": steam_cfg.steam_exe,
            "steam_exe_present": steam_exe_present,
            "steam_runtime_runner": runner_configured.then_some(runner.clone()),
            "steam_runtime_runner_configured": runner_configured,
        }));
    } else {
        cli_println!("Master Steam root : {}", steam_cfg.root_dir.display());
        cli_println!("Wine prefix       : {}", steam_cfg.wine_prefix.display());
        cli_println!("Layout kind       : {}", steam_cfg.layout_kind);
        cli_println!(
            "steam.exe present : {}",
            if steam_exe_present { "yes" } else { "no" }
        );
        if let Some(exe) = &steam_cfg.steam_exe {
            cli_println!("steam.exe path    : {}", exe.display());
        }
        cli_println!(
            "Runtime runner    : {}",
            if runner_configured { runner.as_str() } else { "(unset)" }
        );
    }
    Ok(())
}
