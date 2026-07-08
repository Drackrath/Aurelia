//! `plugins` command handlers.

use crate::commands::common::*;

use anyhow::{Context, Result};
use aurelia::core::config::load_launcher_config;

/// `luxtorpeda enable|disable`: flip the master toggle for the native-engine plugin.
pub(crate) async fn cmd_luxtorpeda_toggle(enable: bool, json: bool) -> Result<()> {
    let mut cfg = load_launcher_config().await.unwrap_or_default();
    cfg.luxtorpeda_enabled = enable;
    cfg.save().await.context("failed saving launcher config")?;

    if json {
        print_json(&serde_json::json!({ "luxtorpeda_enabled": enable }));
    } else if enable {
        cli_println!("Luxtorpeda enabled. Pin a game with `aurelia config game <id> --native-engine`.");
        match &cfg.luxtorpeda_path {
            Some(p) => cli_println!("Using your configured install at {p} (no download)."),
            None => cli_println!("The client downloads automatically on first use (or run `aurelia luxtorpeda install`)."),
        }
        if !cfg!(target_os = "linux") {
            cli_println!("Note: luxtorpeda only runs on Linux.");
        }
    } else {
        cli_println!("Luxtorpeda disabled. Pinned games fall back to native/Proton launch.");
    }
    Ok(())
}

/// `luxtorpeda install|update`: download the latest client into Aurelia's data dir.
pub(crate) async fn cmd_luxtorpeda_install(json: bool) -> Result<()> {
    let cfg = load_launcher_config().await.unwrap_or_default();
    if let Some(p) = &cfg.luxtorpeda_path {
        anyhow::bail!(
            "a custom luxtorpeda path is configured ({p}); Aurelia uses that install and \
             does not download a managed copy. Run `aurelia luxtorpeda path --clear` first \
             to switch to the managed download."
        );
    }
    if !json {
        cli_println!("Downloading luxtorpeda ...");
    }
    let mut last_pct: i64 = -1;
    let mut on_progress = |done: u64, total: u64| {
        if json || total == 0 {
            return;
        }
        let pct = (done.saturating_mul(100) / total) as i64;
        if pct != last_pct {
            last_pct = pct;
            cli_print!("\r  {pct:>3}%  ({} / {})        ", human_bytes(done), human_bytes(total));
        }
    };
    let entry = aurelia::compat::luxtorpeda::install(&mut on_progress)
        .await
        .context("failed installing luxtorpeda")?;
    let installed = aurelia::compat::luxtorpeda::installed(None);
    let version = installed.as_ref().map(|i| i.version.clone()).unwrap_or_default();

    if json {
        print_json(&serde_json::json!({
            "status": "installed",
            "version": version,
            "entry": entry,
        }));
    } else {
        cli_println!("\n  Installed luxtorpeda {version}");
        cli_println!("  Entry: {}", entry.display());
    }
    Ok(())
}

/// `luxtorpeda status`: report enabled state and installed version.
pub(crate) async fn cmd_luxtorpeda_status(json: bool) -> Result<()> {
    let cfg = load_launcher_config().await.unwrap_or_default();
    let custom = cfg.luxtorpeda_path.as_deref().map(std::path::Path::new);
    let installed = aurelia::compat::luxtorpeda::installed(custom);

    if json {
        print_json(&serde_json::json!({
            "enabled": cfg.luxtorpeda_enabled,
            "custom_path": cfg.luxtorpeda_path,
            "installed": installed,
            "linux": cfg!(target_os = "linux"),
        }));
        return Ok(());
    }

    cli_println!("Luxtorpeda native-engine plugin:");
    cli_println!("  Enabled  : {}", cfg.luxtorpeda_enabled);
    match &cfg.luxtorpeda_path {
        Some(p) => cli_println!("  Source   : custom path ({p})"),
        None => cli_println!("  Source   : managed download"),
    }
    match &installed {
        Some(i) => {
            cli_println!("  Installed: {} ({})", i.version, i.entry.display());
        }
        None if cfg.luxtorpeda_path.is_some() => {
            cli_println!("  Installed: NOT FOUND at the configured custom path");
        }
        None => cli_println!("  Installed: no (run `aurelia luxtorpeda install`)"),
    }
    if !cfg!(target_os = "linux") {
        cli_println!("  Note     : luxtorpeda only runs on Linux.");
    } else {
        cli_println!(
            "  Note     : engines run outside the Steam Runtime container; if one fails to \
             find system libraries, prefer Proton for that title."
        );
    }
    Ok(())
}

/// `luxtorpeda path`: set, show, or clear the external luxtorpeda install path.
pub(crate) async fn cmd_luxtorpeda_path(path: Option<String>, clear: bool, json: bool) -> Result<()> {
    let mut cfg = load_launcher_config().await.unwrap_or_default();

    if clear {
        cfg.luxtorpeda_path = None;
        cfg.save().await.context("failed saving launcher config")?;
    } else if let Some(p) = path {
        // Reject anything that isn't actually a luxtorpeda install, so a typo can't
        // silently disable the managed download and then fail only at launch time.
        if aurelia::compat::luxtorpeda::installed(Some(std::path::Path::new(&p))).is_none() {
            anyhow::bail!(
                "'{p}' is not a luxtorpeda install (no toolmanifest.vdf found there or in a subdirectory)"
            );
        }
        cfg.luxtorpeda_path = Some(p);
        cfg.save().await.context("failed saving launcher config")?;
    }
    // No args (and no --clear): fall through to just report the current value.

    if json {
        print_json(&serde_json::json!({ "custom_path": cfg.luxtorpeda_path }));
    } else {
        match &cfg.luxtorpeda_path {
            Some(p) => cli_println!("Custom luxtorpeda path: {p} (managed download disabled)"),
            None => cli_println!("Custom luxtorpeda path: (none — using the managed download)"),
        }
    }
    Ok(())
}

/// `luxtorpeda uninstall`: delete the downloaded payload.
pub(crate) async fn cmd_luxtorpeda_uninstall(json: bool) -> Result<()> {
    let removed = aurelia::compat::luxtorpeda::uninstall().context("failed removing luxtorpeda")?;
    if json {
        print_json(&serde_json::json!({ "status": if removed { "removed" } else { "not_installed" } }));
    } else if removed {
        cli_println!("Removed the luxtorpeda payload.");
    } else {
        cli_println!("Luxtorpeda was not installed.");
    }
    Ok(())
}

/// `umu enable|disable`: flip the master toggle for the umu-launcher plugin.
pub(crate) async fn cmd_umu_toggle(enable: bool, json: bool) -> Result<()> {
    let mut cfg = load_launcher_config().await.unwrap_or_default();
    cfg.umu_enabled = enable;
    cfg.save().await.context("failed saving launcher config")?;

    if json {
        print_json(&serde_json::json!({ "umu_enabled": enable }));
    } else if enable {
        cli_println!("umu-launcher enabled. Pin a game with `aurelia config game <id> --umu`.");
        match &cfg.umu_path {
            Some(p) => cli_println!("Using your configured install at {p} (no download)."),
            None => cli_println!("umu-launcher downloads automatically on first use (or run `aurelia umu install`)."),
        }
        if !cfg!(target_os = "linux") {
            cli_println!("Note: umu-launcher only runs on Linux.");
        }
    } else {
        cli_println!("umu-launcher disabled. Pinned games fall back to native/Proton launch.");
    }
    Ok(())
}

/// `umu install|update`: download the latest umu-launcher into Aurelia's data dir.
pub(crate) async fn cmd_umu_install(json: bool) -> Result<()> {
    let cfg = load_launcher_config().await.unwrap_or_default();
    if let Some(p) = &cfg.umu_path {
        anyhow::bail!(
            "a custom umu path is configured ({p}); Aurelia uses that install and \
             does not download a managed copy. Run `aurelia umu path --clear` first \
             to switch to the managed download."
        );
    }
    if !json {
        cli_println!("Downloading umu-launcher ...");
    }
    let mut last_pct: i64 = -1;
    let mut on_progress = |done: u64, total: u64| {
        if json || total == 0 {
            return;
        }
        let pct = (done.saturating_mul(100) / total) as i64;
        if pct != last_pct {
            last_pct = pct;
            cli_print!("\r  {pct:>3}%  ({} / {})        ", human_bytes(done), human_bytes(total));
        }
    };
    let entry = aurelia::compat::umu::install(&mut on_progress)
        .await
        .context("failed installing umu-launcher")?;
    let installed = aurelia::compat::umu::installed(None);
    let version = installed.as_ref().map(|i| i.version.clone()).unwrap_or_default();

    if json {
        print_json(&serde_json::json!({
            "status": "installed",
            "version": version,
            "entry": entry,
        }));
    } else {
        cli_println!("\n  Installed umu-launcher {version}");
        cli_println!("  Entry: {}", entry.display());
    }
    Ok(())
}

/// `umu status`: report enabled state and installed version.
pub(crate) async fn cmd_umu_status(json: bool) -> Result<()> {
    let cfg = load_launcher_config().await.unwrap_or_default();
    let custom = cfg.umu_path.as_deref().map(std::path::Path::new);
    let installed = aurelia::compat::umu::installed(custom);

    if json {
        print_json(&serde_json::json!({
            "enabled": cfg.umu_enabled,
            "custom_path": cfg.umu_path,
            "installed": installed,
            "linux": cfg!(target_os = "linux"),
        }));
        return Ok(());
    }

    cli_println!("umu-launcher plugin (Proton via umu):");
    cli_println!("  Enabled  : {}", cfg.umu_enabled);
    match &cfg.umu_path {
        Some(p) => cli_println!("  Source   : custom path ({p})"),
        None => cli_println!("  Source   : managed download"),
    }
    match &installed {
        Some(i) => {
            cli_println!("  Installed: {} ({})", i.version, i.entry.display());
        }
        None if cfg.umu_path.is_some() => {
            cli_println!("  Installed: NOT FOUND at the configured custom path");
        }
        None => cli_println!("  Installed: no (run `aurelia umu install`)"),
    }
    if !cfg!(target_os = "linux") {
        cli_println!("  Note     : umu-launcher only runs on Linux.");
    }
    Ok(())
}

/// `umu path`: set, show, or clear the external umu install path.
pub(crate) async fn cmd_umu_path(path: Option<String>, clear: bool, json: bool) -> Result<()> {
    let mut cfg = load_launcher_config().await.unwrap_or_default();

    if clear {
        cfg.umu_path = None;
        cfg.save().await.context("failed saving launcher config")?;
    } else if let Some(p) = path {
        // Reject anything that isn't actually a umu install, so a typo can't silently
        // disable the managed download and then fail only at launch time.
        if aurelia::compat::umu::installed(Some(std::path::Path::new(&p))).is_none() {
            anyhow::bail!(
                "'{p}' is not a umu install (no `umu-run` found there, in a subdirectory, or as the path itself)"
            );
        }
        cfg.umu_path = Some(p);
        cfg.save().await.context("failed saving launcher config")?;
    }
    // No args (and no --clear): fall through to just report the current value.

    if json {
        print_json(&serde_json::json!({ "custom_path": cfg.umu_path }));
    } else {
        match &cfg.umu_path {
            Some(p) => cli_println!("Custom umu path: {p} (managed download disabled)"),
            None => cli_println!("Custom umu path: (none — using the managed download)"),
        }
    }
    Ok(())
}

/// `umu uninstall`: delete the downloaded payload.
pub(crate) async fn cmd_umu_uninstall(json: bool) -> Result<()> {
    let removed = aurelia::compat::umu::uninstall().context("failed removing umu-launcher")?;
    if json {
        print_json(&serde_json::json!({ "status": if removed { "removed" } else { "not_installed" } }));
    } else if removed {
        cli_println!("Removed the umu-launcher payload.");
    } else {
        cli_println!("umu-launcher was not installed.");
    }
    Ok(())
}
