//! `config` command handlers.

use crate::cli::*;
use crate::commands::common::*;

use anyhow::{Context, Result};
use aurelia::core::config::load_launcher_config;
use aurelia::core::config::save_launcher_config;

pub(crate) async fn cmd_config_show(_json: bool) -> Result<()> {
    // The launcher configuration is structured data; it always renders as JSON.
    let config = load_launcher_config().await?;
    print_json(&config);
    Ok(())
}

/// Load config; optionally mutate, save.
async fn view_or_set<T>(
    value: Option<T>,
    mutate: impl FnOnce(&mut aurelia::core::config::LauncherConfig, T),
) -> Result<(aurelia::core::config::LauncherConfig, bool)> {
    let mut config = load_launcher_config().await?;
    let changed = value.is_some();
    if let Some(v) = value {
        mutate(&mut config, v);
        save_launcher_config(&config).await?;
    }
    Ok((config, changed))
}

/// `config presence [online|offline]`: view or set the presence the daemon
/// announces for friends/chat. `offline` is an invisible presence — you appear
/// offline to friends but still sync your friends list and receive chat.
pub(crate) async fn cmd_config_presence(mode: Option<ChatPresenceArg>, json: bool) -> Result<()> {
    use aurelia::core::config::ChatPresence;
    let (config, changed) = view_or_set(mode, |c, m| c.chat_presence = m.into()).await?;
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
    let (config, changed) = view_or_set(lang, |c, lang| {
        let value = lang.trim().to_ascii_lowercase();
        c.language = if value.is_empty() { None } else { Some(value) };
    })
    .await?;
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

/// `config experimental [true|false]`: view or set the experimental-features gate
/// that unlocks `login --openid` and `login --web-token`. See [`ConfigCommand::Experimental`].
pub(crate) async fn cmd_config_experimental(enabled: Option<bool>, json: bool) -> Result<()> {
    let (config, changed) = view_or_set(enabled, |c, value| c.experimental = value).await?;
    // The env var forces experimental on for a single run regardless of the file.
    let env_override = std::env::var_os("AURELIA_EXPERIMENTAL").is_some_and(|v| {
        let v = v.to_string_lossy();
        let v = v.trim();
        !v.is_empty() && !v.eq_ignore_ascii_case("0") && !v.eq_ignore_ascii_case("false")
    });
    let effective = config.experimental || env_override;

    if json {
        print_json(&serde_json::json!({
            "experimental": config.experimental,
            "env_override": env_override,
            "effective": effective,
        }));
    } else {
        cli_println!(
            "Experimental features: {}",
            if config.experimental { "enabled" } else { "disabled" }
        );
        if env_override && !config.experimental {
            cli_println!("(AURELIA_EXPERIMENTAL is set — enabled for this run regardless)");
        }
        cli_println!("Gates: login --openid, login --web-token");
        if changed {
            cli_println!("Saved.");
        }
    }
    Ok(())
}

/// `config session-password [--clear]`: set (or remove) the password that keeps
/// `session.json` encrypted on disk. Loading first decrypts with the current
/// password when the file is already encrypted, so changing the password re-wraps
/// the same session. The password itself is never persisted.
pub(crate) async fn cmd_config_session_password(clear: bool, json: bool) -> Result<()> {
    use aurelia::core::config::{delete_session, load_session, save_session};
    use aurelia::core::session_crypto::cache_password;

    // Prompts for the current password if encrypted.
    let session = load_session()
        .await
        .context("failed loading the current session")?;

    let mut config = load_launcher_config().await?;

    if clear {
        config.encrypt_session = false;
        save_launcher_config(&config)
            .await
            .context("failed saving session-encryption config")?;
        // Rewrite plaintext: drop the encrypted file first.
        delete_session().await?;
        save_session(&session).await?;
        if json {
            print_json(&serde_json::json!({ "encrypt_session": false }));
        } else {
            cli_println!("Session encryption disabled; session.json rewritten as plaintext.");
        }
        return Ok(());
    }

    let password = rpassword::prompt_password("New session password: ")
        .context("failed reading new session password")?;
    if password.is_empty() {
        anyhow::bail!("the session password must not be empty (use --clear to disable encryption)");
    }
    let confirm = rpassword::prompt_password("Confirm session password: ")
        .context("failed reading password confirmation")?;
    if password != confirm {
        anyhow::bail!("passwords do not match");
    }

    cache_password(&password);
    config.encrypt_session = true;
    save_launcher_config(&config)
        .await
        .context("failed saving session-encryption config")?;
    save_session(&session).await?;

    if json {
        print_json(&serde_json::json!({ "encrypt_session": true }));
    } else {
        cli_println!("Session encryption enabled; session.json is now encrypted.");
        cli_println!(
            "Commands will ask for the password once per run; set AURELIA_SESSION_PASSWORD \
             for non-interactive use (required by the session daemon)."
        );
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

/// `config steam-runtime-policy [auto|on|off]`: view or set the global default
/// Steam-integration policy applied when a game's own policy is `auto`. Governs how
/// `aurelia play --steam` provides Steam DRM/Steamworks (host client vs the in-Wine
/// Steam runtime). See [`crate::cli::ConfigCommand::SteamRuntimePolicy`].
pub(crate) async fn cmd_config_steam_runtime_policy(
    policy: Option<SteamRuntimeArg>,
    json: bool,
) -> Result<()> {
    use aurelia::core::models::SteamRuntimePolicy;

    let (config, changed) = view_or_set(policy, |c, arg| c.steam_runtime_policy = arg.into()).await?;

    let label = match config.steam_runtime_policy {
        SteamRuntimePolicy::Auto => {
            "auto (host Steam preferred; in-Wine runtime used under --steam when no host Steam)"
        }
        SteamRuntimePolicy::Enabled => "on (always use the in-Wine Steam runtime)",
        SteamRuntimePolicy::Disabled => "off (host Steam only; never the in-Wine runtime)",
    };
    if json {
        print_json(&serde_json::json!({
            "steam_runtime_policy": config.steam_runtime_policy,
        }));
    } else {
        cli_println!("Steam runtime policy (global default): {label}");
        if changed {
            cli_println!("Saved. Applies to games whose own policy is `auto`.");
        }
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

/// `config clear-games`: reset every game's per-game settings (both stores).
pub(crate) async fn cmd_config_clear_games(yes: bool, json: bool) -> Result<()> {
    let mut cfg = load_launcher_config().await?;
    let mut user_configs = aurelia::core::config::load_user_configs().await?;
    let ids: std::collections::BTreeSet<u32> = cfg
        .game_configs
        .keys()
        .chain(user_configs.keys())
        .copied()
        .collect();
    if ids.is_empty() {
        if json {
            print_json(&serde_json::json!({ "cleared": 0 }));
        } else {
            cli_println!("No per-game settings to clear.");
        }
        return Ok(());
    }
    crate::commands::common::confirm_write(
        "clear",
        &format!(
            "About to reset the per-game settings of {} game(s) to defaults. Continue? [y/N] ",
            ids.len()
        ),
        yes,
        json,
    )?;
    cfg.game_configs.clear();
    user_configs.clear();
    cfg.save().await.context("failed saving game config")?;
    aurelia::core::config::save_user_configs(&user_configs)
        .await
        .context("failed saving per-game config")?;
    if json {
        print_json(&serde_json::json!({ "cleared": ids.len() }));
    } else {
        cli_println!("Cleared the per-game settings of {} game(s).", ids.len());
    }
    Ok(())
}

/// Which platform payloads are installed: (linux, windows).
async fn installed_platform_set(app_id: u32) -> Option<(bool, bool)> {
    let installed = aurelia::library::scan_installed_app_info().await.ok()?;
    let info = installed.get(&app_id)?;
    crate::commands::library::detect_installed_platform_set(&info.install_path.to_string_lossy())
}

/// `config game`: view or set a game's per-game launch settings.
pub(crate) async fn cmd_config_game(args: GameConfigArgs, json: bool) -> Result<()> {
    let GameConfigArgs {
        app_id,
        proton,
        clear_proton,
        platform,
        no_platform,
        clear,
        native_engine,
        no_native_engine,
        umu,
        no_umu,
        launch_script,
        no_launch_script,
        steam_runtime,
        steam_prefix_mode,
    } = args;
    use aurelia::core::config::GameRunner;
    use aurelia::core::models::{SteamPrefixMode, SteamRuntimePolicy};

    // The Steam-runtime knobs live in a separate per-game store (user_apps.json) from the
    // GameConfig fields above (config.json). Update whichever store each flag targets.
    let mut user_configs = aurelia::core::config::load_user_configs().await?;
    let mut user_changed = false;
    if clear {
        user_changed = user_configs.remove(&app_id).is_some();
    } else {
        let ua = user_configs.entry(app_id).or_default();
        if let Some(sr) = steam_runtime {
            ua.steam_runtime_policy = sr.into();
            user_changed = true;
        }
        if let Some(pm) = steam_prefix_mode {
            ua.steam_prefix_mode = pm.into();
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
    if clear {
        changed = cfg.game_configs.remove(&app_id).is_some();
    } else {
        let entry = cfg.game_configs.entry(app_id).or_default();
        if clear_proton {
            entry.forced_proton_version = None;
            changed = true;
        } else if let Some(p) = proton {
            entry.forced_proton_version = Some(p);
            changed = true;
        }
        if no_platform {
            entry.platform_preference = None;
            changed = true;
        } else if let Some(pl) = platform {
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
    let ua = user_configs.get(&app_id).cloned().unwrap_or_default();
    let platforms = installed_platform_set(app_id).await;
    let platform_resolved = match platforms {
        Some((true, true)) => "linux + windows installed",
        Some((true, false)) => "linux",
        Some((false, true)) => "windows",
        Some((false, false)) => "no recognizable payload",
        None => "unknown until installed",
    };
    let runner_label = match entry.runner {
        GameRunner::Auto => match (platforms, entry.platform_preference.as_deref()) {
            (Some((true, true)), Some("linux")) => "auto (Native, by linux preference)",
            (Some((true, true)), Some("windows")) => {
                "auto (Proton via Wine-TKG, by windows preference)"
            }
            (Some((true, true)), _) => "auto (Native or Proton, no platform preference)",
            (Some((true, false)), _) => "auto (Native)",
            (Some((false, true)), _) => "auto (Proton via Wine-TKG)",
            _ => "auto (resolved at launch)",
        }
        .to_string(),
        GameRunner::Luxtorpeda => "luxtorpeda (native engine)".to_string(),
        GameRunner::Umu => "umu (Proton via umu-launcher)".to_string(),
    };
    let steam_runtime_label = match ua.steam_runtime_policy {
        SteamRuntimePolicy::Auto if ua.use_steam_runtime => {
            "auto (on: legacy per-game flag)".to_string()
        }
        SteamRuntimePolicy::Auto => match cfg.steam_runtime_policy {
            SteamRuntimePolicy::Enabled => "auto (on, from global)".to_string(),
            SteamRuntimePolicy::Disabled => "auto (off, from global)".to_string(),
            SteamRuntimePolicy::Auto => {
                "auto (global auto: bridges host Steam if running, else standalone)".to_string()
            }
        },
        SteamRuntimePolicy::Enabled => "on".to_string(),
        SteamRuntimePolicy::Disabled => "off".to_string(),
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
        let proton_default = format!("(global default: {})", cfg.proton_version);
        cli_println!(
            "  Proton       : {}",
            entry.forced_proton_version.as_deref().unwrap_or(&proton_default)
        );
        let platform_auto = format!("(auto: {platform_resolved})");
        cli_println!(
            "  Platform     : {}",
            entry.platform_preference.as_deref().unwrap_or(&platform_auto)
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
