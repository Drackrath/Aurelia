//! `plugins` command handlers (luxtorpeda, umu-launcher).
//!
//! Both plugins expose the identical command surface (enable/disable, install,
//! status, path, uninstall); one generic handler set is driven by a
//! [`PluginCmd`] descriptor per plugin so wording and behavior can't drift.

use crate::commands::common::*;

use anyhow::{Context, Result};
use aurelia::compat::plugin::InstalledPlugin;
use aurelia::core::config::{load_launcher_config, LauncherConfig};
use std::path::{Path, PathBuf};
use std::pin::Pin;

type InstallFn =
    for<'a> fn(
        &'a mut (dyn FnMut(u64, u64) + Send),
    ) -> Pin<Box<dyn Future<Output = Result<PathBuf>> + Send + 'a>>;

/// Command-side description of an optional plugin.
struct PluginCmd {
    /// CLI command name (`aurelia <slug> ...`), also the config-path noun.
    slug: &'static str,
    /// Name leading a sentence ("Luxtorpeda", "umu-launcher").
    name: &'static str,
    /// Project name mid-sentence ("luxtorpeda", "umu-launcher").
    project: &'static str,
    /// `config game` flag that pins a game to this plugin.
    pin_flag: &'static str,
    /// Subject of the auto-download hint in `enable` output.
    download_subject: &'static str,
    /// Header line for `status`.
    status_header: &'static str,
    /// Extra `status` note shown on Linux, if any.
    linux_note: Option<&'static str>,
    /// JSON key for the enabled flag.
    enabled_key: &'static str,
    /// Reason text for a rejected `path` argument.
    bad_path_reason: &'static str,
    enabled: fn(&LauncherConfig) -> bool,
    set_enabled: fn(&mut LauncherConfig, bool),
    path_of: fn(&LauncherConfig) -> Option<&String>,
    set_path: fn(&mut LauncherConfig, Option<String>),
    installed: fn(Option<&Path>) -> Option<InstalledPlugin>,
    install: InstallFn,
    uninstall: fn() -> Result<bool>,
}

static LUXTORPEDA: PluginCmd = PluginCmd {
    slug: "luxtorpeda",
    name: "Luxtorpeda",
    project: "luxtorpeda",
    pin_flag: "--native-engine",
    download_subject: "The client",
    status_header: "Luxtorpeda native-engine plugin:",
    linux_note: Some(
        "engines run outside the Steam Runtime container; if one fails to \
         find system libraries, prefer Proton for that title.",
    ),
    enabled_key: "luxtorpeda_enabled",
    bad_path_reason: "no toolmanifest.vdf found there or in a subdirectory",
    enabled: |cfg| cfg.luxtorpeda_enabled,
    set_enabled: |cfg, v| cfg.luxtorpeda_enabled = v,
    path_of: |cfg| cfg.luxtorpeda_path.as_ref(),
    set_path: |cfg, v| cfg.luxtorpeda_path = v,
    installed: aurelia::compat::luxtorpeda::installed,
    install: |on_progress| Box::pin(aurelia::compat::luxtorpeda::install(on_progress)),
    uninstall: aurelia::compat::luxtorpeda::uninstall,
};

static UMU: PluginCmd = PluginCmd {
    slug: "umu",
    name: "umu-launcher",
    project: "umu-launcher",
    pin_flag: "--umu",
    download_subject: "umu-launcher",
    status_header: "umu-launcher plugin (Proton via umu):",
    linux_note: None,
    enabled_key: "umu_enabled",
    bad_path_reason: "no `umu-run` found there, in a subdirectory, or as the path itself",
    enabled: |cfg| cfg.umu_enabled,
    set_enabled: |cfg, v| cfg.umu_enabled = v,
    path_of: |cfg| cfg.umu_path.as_ref(),
    set_path: |cfg, v| cfg.umu_path = v,
    installed: aurelia::compat::umu::installed,
    install: |on_progress| Box::pin(aurelia::compat::umu::install(on_progress)),
    uninstall: aurelia::compat::umu::uninstall,
};

/// `<plugin> enable|disable`: flip the master toggle.
async fn plugin_toggle(p: &PluginCmd, enable: bool, json: bool) -> Result<()> {
    let mut cfg = load_launcher_config().await?;
    (p.set_enabled)(&mut cfg, enable);
    cfg.save().await.context("failed saving launcher config")?;

    if json {
        let mut obj = serde_json::Map::new();
        obj.insert(p.enabled_key.to_string(), enable.into());
        print_json(&serde_json::Value::Object(obj));
    } else if enable {
        cli_println!(
            "{} enabled. Pin a game with `aurelia config game <id> {}`.",
            p.name,
            p.pin_flag
        );
        match (p.path_of)(&cfg) {
            Some(path) => cli_println!("Using your configured install at {path} (no download)."),
            None => cli_println!(
                "{} downloads automatically on first use (or run `aurelia {} install`).",
                p.download_subject,
                p.slug
            ),
        }
        if !cfg!(target_os = "linux") {
            cli_println!("Note: {} only runs on Linux.", p.project);
        }
    } else {
        cli_println!("{} disabled. Pinned games fall back to native/Proton launch.", p.name);
    }
    Ok(())
}

/// `<plugin> install|update`: download the latest release into Aurelia's data dir.
async fn plugin_install(p: &PluginCmd, json: bool) -> Result<()> {
    let cfg = load_launcher_config().await?;
    if let Some(path) = (p.path_of)(&cfg) {
        anyhow::bail!(
            "a custom {} path is configured ({path}); Aurelia uses that install and \
             does not download a managed copy. Run `aurelia {} path --clear` first \
             to switch to the managed download.",
            p.slug,
            p.slug
        );
    }
    if !json {
        cli_println!("Downloading {} ...", p.project);
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
    let entry = (p.install)(&mut on_progress)
        .await
        .with_context(|| format!("failed installing {}", p.project))?;
    let installed = (p.installed)(None);
    let version = installed.as_ref().map(|i| i.version.clone()).unwrap_or_default();

    if json {
        print_json(&serde_json::json!({
            "status": "installed",
            "version": version,
            "entry": entry,
        }));
    } else {
        cli_println!("\n  Installed {} {version}", p.project);
        cli_println!("  Entry: {}", entry.display());
    }
    Ok(())
}

/// `<plugin> status`: report enabled state and installed version.
async fn plugin_status(p: &PluginCmd, json: bool) -> Result<()> {
    let cfg = load_launcher_config().await?;
    let custom_path = (p.path_of)(&cfg);
    let custom = custom_path.map(Path::new);
    let installed = (p.installed)(custom);

    if json {
        print_json(&serde_json::json!({
            "enabled": (p.enabled)(&cfg),
            "custom_path": custom_path,
            "installed": installed,
            "linux": cfg!(target_os = "linux"),
        }));
        return Ok(());
    }

    cli_println!("{}", p.status_header);
    cli_println!("  Enabled  : {}", (p.enabled)(&cfg));
    match custom_path {
        Some(path) => cli_println!("  Source   : custom path ({path})"),
        None => cli_println!("  Source   : managed download"),
    }
    match &installed {
        Some(i) => {
            cli_println!("  Installed: {} ({})", i.version, i.entry.display());
        }
        None if custom_path.is_some() => {
            cli_println!("  Installed: NOT FOUND at the configured custom path");
        }
        None => cli_println!("  Installed: no (run `aurelia {} install`)", p.slug),
    }
    if !cfg!(target_os = "linux") {
        cli_println!("  Note     : {} only runs on Linux.", p.project);
    } else if let Some(note) = p.linux_note {
        cli_println!("  Note     : {note}");
    }
    Ok(())
}

/// `<plugin> path`: set, show, or clear the external install path.
async fn plugin_path(p: &PluginCmd, path: Option<String>, clear: bool, json: bool) -> Result<()> {
    let mut cfg = load_launcher_config().await?;

    if clear {
        (p.set_path)(&mut cfg, None);
        cfg.save().await.context("failed saving launcher config")?;
    } else if let Some(new_path) = path {
        // Reject anything that isn't actually an install of this plugin, so a typo
        // can't silently disable the managed download and then fail only at launch
        // time.
        if (p.installed)(Some(Path::new(&new_path))).is_none() {
            anyhow::bail!("'{new_path}' is not a {} install ({})", p.slug, p.bad_path_reason);
        }
        (p.set_path)(&mut cfg, Some(new_path));
        cfg.save().await.context("failed saving launcher config")?;
    }
    // No args (and no --clear): fall through to just report the current value.

    let current = (p.path_of)(&cfg);
    if json {
        print_json(&serde_json::json!({ "custom_path": current }));
    } else {
        match current {
            Some(path) => {
                cli_println!("Custom {} path: {path} (managed download disabled)", p.slug)
            }
            None => cli_println!("Custom {} path: (none — using the managed download)", p.slug),
        }
    }
    Ok(())
}

/// `<plugin> uninstall`: delete the downloaded payload.
async fn plugin_uninstall(p: &PluginCmd, json: bool) -> Result<()> {
    let removed = (p.uninstall)().with_context(|| format!("failed removing {}", p.project))?;
    if json {
        print_json(&serde_json::json!({ "status": if removed { "removed" } else { "not_installed" } }));
    } else if removed {
        cli_println!("Removed the {} payload.", p.project);
    } else {
        cli_println!("{} was not installed.", p.name);
    }
    Ok(())
}

pub(crate) async fn cmd_luxtorpeda_toggle(enable: bool, json: bool) -> Result<()> {
    plugin_toggle(&LUXTORPEDA, enable, json).await
}

pub(crate) async fn cmd_luxtorpeda_install(json: bool) -> Result<()> {
    plugin_install(&LUXTORPEDA, json).await
}

pub(crate) async fn cmd_luxtorpeda_status(json: bool) -> Result<()> {
    plugin_status(&LUXTORPEDA, json).await
}

pub(crate) async fn cmd_luxtorpeda_path(path: Option<String>, clear: bool, json: bool) -> Result<()> {
    plugin_path(&LUXTORPEDA, path, clear, json).await
}

pub(crate) async fn cmd_luxtorpeda_uninstall(json: bool) -> Result<()> {
    plugin_uninstall(&LUXTORPEDA, json).await
}

pub(crate) async fn cmd_umu_toggle(enable: bool, json: bool) -> Result<()> {
    plugin_toggle(&UMU, enable, json).await
}

pub(crate) async fn cmd_umu_install(json: bool) -> Result<()> {
    plugin_install(&UMU, json).await
}

pub(crate) async fn cmd_umu_status(json: bool) -> Result<()> {
    plugin_status(&UMU, json).await
}

pub(crate) async fn cmd_umu_path(path: Option<String>, clear: bool, json: bool) -> Result<()> {
    plugin_path(&UMU, path, clear, json).await
}

pub(crate) async fn cmd_umu_uninstall(json: bool) -> Result<()> {
    plugin_uninstall(&UMU, json).await
}
