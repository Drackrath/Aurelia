//! `config` command handlers.

use crate::cli::*;
use crate::commands::common::*;

use std::path::PathBuf;
use anyhow::{Context, Result};
use aurelia::core::config::load_launcher_config;
use aurelia::core::config::save_launcher_config;

pub(crate) async fn cmd_config_show(_json: bool) -> Result<()> {
    // The launcher configuration is structured data; it always renders as JSON.
    let config = load_launcher_config().await.unwrap_or_default();
    cli_println!("{}", serde_json::to_string_pretty(&config)?);
    Ok(())
}

/// `config presence [online|offline]`: view or set the presence the daemon
/// announces for friends/chat. `offline` is an invisible presence — you appear
/// offline to friends but still sync your friends list and receive chat.
pub(crate) async fn cmd_config_presence(mode: Option<ChatPresenceArg>, json: bool) -> Result<()> {
    use aurelia::core::config::ChatPresence;
    let mut config = load_launcher_config().await.unwrap_or_default();
    let changed = mode.is_some();
    if let Some(mode) = mode {
        config.chat_presence = mode.into();
        save_launcher_config(&config).await?;
    }
    let current = match config.chat_presence {
        ChatPresence::Online => "online",
        ChatPresence::Offline => "offline",
    };
    if json {
        print_json(&serde_json::json!({ "chat_presence": current }));
    } else {
        cli_println!("Chat presence: {current}");
        if changed {
            cli_println!(
                "Restart the session daemon for this to take effect (`aurelia daemon stop` or `aurelia kill`)."
            );
        }
    }
    Ok(())
}

/// `config language [<name>]`: view or set the default Steam API language name
/// used by `aurelia achievements` when `--lang` is not given. Pass an empty
/// value to clear it (falling back to English).
pub(crate) async fn cmd_config_language(lang: Option<String>, json: bool) -> Result<()> {
    let mut config = load_launcher_config().await.unwrap_or_default();
    let changed = lang.is_some();
    if let Some(lang) = lang {
        let value = lang.trim().to_ascii_lowercase();
        config.language = if value.is_empty() { None } else { Some(value) };
        save_launcher_config(&config).await?;
    }
    let current = config.language.as_deref();
    if json {
        print_json(&serde_json::json!({ "language": current }));
    } else {
        match current {
            Some(lang) => cli_println!("Language: {lang}"),
            None => cli_println!("Language: english (default)"),
        }
        if changed {
            cli_println!("Saved.");
        }
    }
    Ok(())
}

/// `config protons`: list the Proton/Wine runtimes actually installed on disk.
/// Shares discovery with `proton list --installed` (no hardcoded placeholders).
pub(crate) async fn cmd_config_protons(json: bool) -> Result<()> {
    let cfg = load_launcher_config().await.unwrap_or_default();
    let installed = aurelia::compat::proton::list_installed(std::path::Path::new(&cfg.steam_library_path));
    let steam: Vec<&str> = installed
        .iter()
        .filter(|i| i.location == "steam")
        .map(|i| i.name.as_str())
        .collect();
    let custom: Vec<&str> = installed
        .iter()
        .filter(|i| i.location == "custom")
        .map(|i| i.name.as_str())
        .collect();

    if json {
        print_json(&serde_json::json!({
            "steam": steam,
            "custom": custom,
            "default": cfg.proton_version,
        }));
        return Ok(());
    }

    if installed.is_empty() {
        cli_println!("No Proton/Wine runtimes installed.");
        cli_println!("Install one with `aurelia proton install <NAME>` (see `aurelia proton list`).");
        return Ok(());
    }
    if !steam.is_empty() {
        cli_println!("Steam runtimes:");
        for s in &steam {
            cli_println!("  {s}");
        }
    }
    if !custom.is_empty() {
        cli_println!("Custom (compatibilitytools.d):");
        for c in &custom {
            cli_println!("  {c}");
        }
    }
    Ok(())
}

/// `config game`: view or set a game's per-game launch settings.
pub(crate) async fn cmd_config_game(
    app_id: u32,
    proton: Option<String>,
    clear_proton: bool,
    platform: Option<PlatformArg>,
    native_engine: bool,
    no_native_engine: bool,
    umu: bool,
    no_umu: bool,
    launch_script: Option<PathBuf>,
    no_launch_script: bool,
    json: bool,
) -> Result<()> {
    use aurelia::core::config::GameRunner;

    let mut cfg = load_launcher_config().await.unwrap_or_default();
    let mut changed = false;
    {
        let entry = cfg.game_configs.entry(app_id).or_default();
        if clear_proton {
            entry.forced_proton_version = None;
            changed = true;
        } else if let Some(p) = proton {
            entry.forced_proton_version = Some(p);
            changed = true;
        }
        if let Some(pl) = platform {
            entry.platform_preference = Some(
                match pl {
                    PlatformArg::Windows => "windows",
                    PlatformArg::Linux => "linux",
                }
                .to_string(),
            );
            changed = true;
        }
        if native_engine {
            entry.runner = GameRunner::Luxtorpeda;
            changed = true;
        } else if no_native_engine {
            entry.runner = GameRunner::Auto;
            changed = true;
        } else if umu {
            entry.runner = GameRunner::Umu;
            changed = true;
        } else if no_umu {
            entry.runner = GameRunner::Auto;
            changed = true;
        }
        if no_launch_script {
            entry.launch_script = None;
            changed = true;
        } else if let Some(s) = launch_script {
            entry.launch_script = Some(s.to_string_lossy().to_string());
            changed = true;
        }
    }
    if changed {
        cfg.save().await.context("failed saving game config")?;
    }

    let entry = cfg.game_configs.get(&app_id).cloned().unwrap_or_default();
    let runner_label = match entry.runner {
        GameRunner::Auto => "auto",
        GameRunner::Luxtorpeda => "luxtorpeda (native engine)",
        GameRunner::Umu => "umu (Proton via umu-launcher)",
    };
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "forced_proton_version": entry.forced_proton_version,
            "platform_preference": entry.platform_preference,
            "runner": entry.runner,
            "launch_script": entry.launch_script,
        }));
    } else {
        cli_println!("App {app_id}:");
        cli_println!(
            "  Proton  : {}",
            entry.forced_proton_version.as_deref().unwrap_or("(global default)")
        );
        cli_println!(
            "  Platform: {}",
            entry.platform_preference.as_deref().unwrap_or("(auto)")
        );
        cli_println!("  Runner  : {runner_label}");
        cli_println!(
            "  Script  : {}",
            entry.launch_script.as_deref().unwrap_or("(auto-detected / none)")
        );
    }
    Ok(())
}
