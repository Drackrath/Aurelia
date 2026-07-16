//! `config` command handlers.

use crate::cli::*;
use crate::commands::common::*;

use std::path::PathBuf;
use anyhow::{Context, Result};
use aurelia::core::config::load_launcher_config;
use aurelia::core::config::save_launcher_config;

pub(crate) async fn cmd_config_show(_json: bool) -> Result<()> {
    // The launcher configuration is structured data; it always renders as JSON.
    let config = load_launcher_config().await?;
    cli_println!("{}", serde_json::to_string_pretty(&config)?);
    Ok(())
}

/// `config presence [online|offline]`: view or set the presence the daemon
/// announces for friends/chat. `offline` is an invisible presence — you appear
/// offline to friends but still sync your friends list and receive chat.
pub(crate) async fn cmd_config_presence(mode: Option<ChatPresenceArg>, json: bool) -> Result<()> {
    use aurelia::core::config::ChatPresence;
    let mut config = load_launcher_config().await?;
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
    let mut config = load_launcher_config().await?;
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

/// `config proxy [<url>] [--no-proxy <list>] [--clear]`: view or set the network
/// proxy used for all HTTP(S) communication (Steam web endpoints, depot downloads, and
/// Proton/plugin release lookups). With no arguments, prints the current setting.
pub(crate) async fn cmd_config_proxy(
    url: Option<String>,
    no_proxy: Option<String>,
    clear: bool,
    json: bool,
) -> Result<()> {
    use aurelia::core::net::validate_proxy_url;

    let mut config = load_launcher_config().await?;
    let changed = clear || url.is_some() || no_proxy.is_some();

    if clear {
        config.proxy.url = None;
        config.proxy.no_proxy = None;
    } else {
        if let Some(url) = url {
            let value = url.trim();
            if value.is_empty() {
                config.proxy.url = None;
            } else {
                validate_proxy_url(value)?;
                config.proxy.url = Some(value.to_string());
            }
        }
        if let Some(no_proxy) = no_proxy {
            let value = no_proxy.trim();
            config.proxy.no_proxy = (!value.is_empty()).then(|| value.to_string());
        }
    }

    if changed {
        save_launcher_config(&config).await.context("failed saving proxy config")?;
    }

    if json {
        print_json(&serde_json::json!({
            "url": config.proxy.url,
            "no_proxy": config.proxy.no_proxy,
        }));
    } else {
        match config.proxy.url.as_deref() {
            Some(url) => cli_println!("Proxy: {url}"),
            None => cli_println!("Proxy: (none — direct connection)"),
        }
        if let Some(no_proxy) = config.proxy.no_proxy.as_deref() {
            cli_println!("Bypass: {no_proxy}");
        }
        if changed {
            cli_println!(
                "Saved. Takes effect on the next command; restart the session daemon (`aurelia daemon stop`) to apply it there."
            );
        }
    }
    Ok(())
}

/// `config steam-runtime-runner [<name>]`: view or set the Wine/Proton runner that
/// hosts the Windows Steam runtime (`steam-runtime install`/`repair`). Pass an empty
/// string to clear it. On set, the value is resolved against the installed runtimes so
/// a typo is caught immediately rather than at install time.
pub(crate) async fn cmd_config_steam_runtime_runner(
    runner: Option<String>,
    json: bool,
) -> Result<()> {
    use std::path::PathBuf;

    let mut config = load_launcher_config().await?;
    let changed = runner.is_some();

    if let Some(runner) = runner {
        let value = runner.trim();
        config.steam_runtime_runner = PathBuf::from(value);
        save_launcher_config(&config).await?;
    }

    let current = config.steam_runtime_runner.to_string_lossy().to_string();
    let configured = !current.is_empty();

    // Soft validation: resolve the saved name to a bare Wine binary so the user learns
    // now (not at install time) whether it points at something usable. `resolve_steam_
    // runtime_wine` resolves quietly (no stray log) and returns Err when the name matches
    // no installed runtime — mirroring how `proton default` warns on an uninstalled pick.
    let library_root = PathBuf::from(&config.steam_library_path);
    let resolved = if configured {
        aurelia::core::utils::resolve_steam_runtime_wine(&current, &library_root).ok()
    } else {
        None
    };

    if json {
        print_json(&serde_json::json!({
            "steam_runtime_runner": configured.then(|| current.clone()),
            "resolved_wine": resolved.as_ref().map(|p: &PathBuf| p.display().to_string()),
        }));
        return Ok(());
    }

    match (configured, &resolved) {
        (false, _) => {
            cli_println!("Steam runtime runner: (unset)");
            cli_println!(
                "Set one with `aurelia config steam-runtime-runner <NAME>` — see \
                 `aurelia proton list` for installed runtime names (e.g. GE-Proton9-20)."
            );
        }
        (true, Some(wine)) => {
            cli_println!("Steam runtime runner: {current}");
            cli_println!("Resolves to bare Wine   : {}", wine.display());
        }
        (true, None) => {
            cli_println!("Steam runtime runner: {current}");
            cli_eprintln!(
                "Warning: '{current}' does not resolve to an installed Wine/Proton runtime yet. \
                 Install it (`aurelia proton install {current}`) or pick another — \
                 see `aurelia proton list`."
            );
        }
    }
    if changed {
        cli_println!("Saved.");
    }
    Ok(())
}

/// `config protons`: list the Proton/Wine runtimes actually installed on disk.
/// Shares discovery with `proton list --installed` (no hardcoded placeholders).
pub(crate) async fn cmd_config_protons(json: bool) -> Result<()> {
    let cfg = load_launcher_config().await?;
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
#[allow(clippy::too_many_arguments)]
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
    steam_runtime: Option<SteamRuntimeArg>,
    steam_prefix_mode: Option<SteamPrefixModeArg>,
    json: bool,
) -> Result<()> {
    use aurelia::core::config::GameRunner;
    use aurelia::core::models::{SteamPrefixMode, SteamRuntimePolicy};

    // The Steam-runtime knobs live in a separate per-game store (user_apps.json) from the
    // GameConfig fields above (config.json). Update whichever store each flag targets.
    let mut user_configs = aurelia::core::config::load_user_configs().await?;
    let mut user_changed = false;
    {
        let ua = user_configs.entry(app_id).or_default();
        if let Some(sr) = steam_runtime {
            ua.steam_runtime_policy = match sr {
                SteamRuntimeArg::Auto => SteamRuntimePolicy::Auto,
                SteamRuntimeArg::On => SteamRuntimePolicy::Enabled,
                SteamRuntimeArg::Off => SteamRuntimePolicy::Disabled,
            };
            user_changed = true;
        }
        if let Some(pm) = steam_prefix_mode {
            ua.steam_prefix_mode = match pm {
                SteamPrefixModeArg::Shared => SteamPrefixMode::Shared,
                SteamPrefixModeArg::PerGame => SteamPrefixMode::PerGame,
            };
            user_changed = true;
        }
    }
    if user_changed {
        aurelia::core::config::save_user_configs(&user_configs)
            .await
            .context("failed saving per-game Steam-runtime config")?;
    }

    let mut cfg = load_launcher_config().await?;
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
    let ua = user_configs.get(&app_id).cloned().unwrap_or_default();
    let steam_runtime_label = match ua.steam_runtime_policy {
        SteamRuntimePolicy::Auto => "auto (off)",
        SteamRuntimePolicy::Enabled => "on",
        SteamRuntimePolicy::Disabled => "off",
    };
    let prefix_mode_label = match ua.steam_prefix_mode {
        SteamPrefixMode::Shared => "shared",
        SteamPrefixMode::PerGame => "per-game",
    };
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "forced_proton_version": entry.forced_proton_version,
            "platform_preference": entry.platform_preference,
            "runner": entry.runner,
            "launch_script": entry.launch_script,
            "steam_runtime_policy": ua.steam_runtime_policy,
            "steam_prefix_mode": ua.steam_prefix_mode,
        }));
    } else {
        cli_println!("App {app_id}:");
        cli_println!(
            "  Proton       : {}",
            entry.forced_proton_version.as_deref().unwrap_or("(global default)")
        );
        cli_println!(
            "  Platform     : {}",
            entry.platform_preference.as_deref().unwrap_or("(auto)")
        );
        cli_println!("  Runner       : {runner_label}");
        cli_println!(
            "  Script       : {}",
            entry.launch_script.as_deref().unwrap_or("(auto-detected / none)")
        );
        cli_println!("  Steam runtime: {steam_runtime_label}");
        cli_println!("  Prefix mode  : {prefix_mode_label}");
    }
    Ok(())
}
