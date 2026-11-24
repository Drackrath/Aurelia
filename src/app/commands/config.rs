//! `config` command handlers.

use crate::app::cli::*;
use crate::app::commands::common::*;

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

/// View or set persona wait.
pub(crate) async fn cmd_config_persona_wait(seconds: Option<u64>, json: bool) -> Result<()> {
    let (config, changed) = view_or_set(seconds, |c, s| c.persona_wait_secs = s).await?;
    let current = config.persona_wait_secs;
    if json {
        print_json(&serde_json::json!({ "persona_wait_secs": current }));
    } else {
        cli_println!("Persona wait: {current}s");
        if changed {
            cli_println!("Saved.");
        }
    }
    Ok(())
}

/// View or set the price country.
pub(crate) async fn cmd_config_country(country: Option<String>, json: bool) -> Result<()> {
    use aurelia::core::error::{ErrorKind, TypedError};
    use aurelia::core::locale::normalize_country;
    let value = match country {
        None => None,
        Some(raw) if raw.trim().is_empty() => Some(None),
        Some(raw) => Some(Some(normalize_country(&raw).ok_or_else(|| {
            TypedError::new(
                ErrorKind::InvalidInput,
                format!("invalid country code `{raw}` — use a two-letter ISO code like US or DE"),
            )
        })?)),
    };
    let (config, changed) = view_or_set(value, |c, cc| c.country = cc).await?;
    let current = config.country.as_deref();
    if json {
        print_json(&serde_json::json!({ "country": current }));
    } else {
        match current {
            Some(cc) => cli_println!("Country: {cc}"),
            None => cli_println!("Country: auto (system locale, else US)"),
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
            if config.experimental {
                "enabled"
            } else {
                "disabled"
            }
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
        save_launcher_config(&config)
            .await
            .context("failed saving proxy config")?;
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
        aurelia::core::runner::resolve_steam_runtime_wine(&current, &library_root).ok()
    } else {
        None
    };

    if json {
        print_json(&serde_json::json!({
            "steam_runtime_runner": configured.then(|| current.clone()),
            "resolved_wine": resolved.as_ref().map(|p: &PathBuf| p.display().to_string()),
            "steam_runtime_supported": aurelia::core::runner::STEAM_RUNTIME_SUPPORTED,
        }));
        return Ok(());
    }
    if !aurelia::core::runner::STEAM_RUNTIME_SUPPORTED {
        cli_eprintln!(
            "Note: {}",
            aurelia::core::runner::steam_runtime_unsupported_msg()
        );
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
/// Steam runtime). See [`crate::app::cli::ConfigCommand::SteamRuntimePolicy`].
/// `config shader-precache [auto|off]`.
pub(crate) async fn cmd_config_shader_precache(
    mode: Option<ShaderPrecacheArg>,
    json: bool,
) -> Result<()> {
    use aurelia::launch::shaders::ShaderPrecache;
    let (config, changed) = view_or_set(mode, |c, m| c.shader_precache = m.into()).await?;
    if json {
        print_json(&serde_json::json!({ "shader_precache": config.shader_precache }));
        return Ok(());
    }
    let label = match config.shader_precache {
        ShaderPrecache::Auto => {
            "auto (replay the Steam pipeline cache before launch and share Steam's shader caches)"
        }
        ShaderPrecache::Off => "off (shaders compile while the game runs)",
    };
    cli_println!("Shader pre-caching: {label}");
    if changed {
        cli_println!("Saved. Applies to the next launch.");
    }
    Ok(())
}

/// `config hide-uninstalled-launch-options [on|off]`.
pub(crate) async fn cmd_config_hide_uninstalled_launch_options(
    mode: Option<OnOffArg>,
    json: bool,
) -> Result<()> {
    let (config, changed) = view_or_set(mode, |c, m| {
        c.hide_uninstalled_launch_options = matches!(m, OnOffArg::On);
    })
    .await?;
    if json {
        print_json(&serde_json::json!({
            "hide_uninstalled_launch_options": config.hide_uninstalled_launch_options
        }));
        return Ok(());
    }
    let label = if config.hide_uninstalled_launch_options {
        "on (`launch-options` lists only installed entries of installed games)"
    } else {
        "off (`launch-options` lists every entry; `play` refuses ones not installed)"
    };
    cli_println!("Hide uninstalled launch options: {label}");
    if changed {
        cli_println!("Saved.");
    }
    Ok(())
}

/// `config cloud-sync [on|off]`.
pub(crate) async fn cmd_config_cloud_sync(mode: Option<OnOffArg>, json: bool) -> Result<()> {
    let (config, changed) = view_or_set(mode, |c, m| {
        c.enable_cloud_sync = matches!(m, OnOffArg::On);
    })
    .await?;
    let overrides: std::collections::BTreeMap<u32, bool> = config
        .game_configs
        .iter()
        .filter_map(|(id, g)| g.cloud_sync.map(|on| (*id, on)))
        .collect();
    if json {
        print_json(&serde_json::json!({
            "enable_cloud_sync": config.enable_cloud_sync,
            "game_overrides": overrides,
        }));
        return Ok(());
    }
    let label = if config.enable_cloud_sync {
        "on (`play` syncs Steam Cloud saves down before and up after the game)"
    } else {
        "off (`play` leaves Steam Cloud alone; `cloud sync` still works)"
    };
    cli_println!("Cloud sync on play: {label}");
    for (id, on) in &overrides {
        cli_println!("  App {id}: {} (per-game)", if *on { "on" } else { "off" });
    }
    if changed {
        cli_println!("Saved. Applies to the next launch.");
    }
    Ok(())
}

/// Print a display target.
fn print_display_target(indent: &str, label: &str, t: &aurelia::core::config::DisplayTarget) {
    if t.is_empty() {
        cli_println!("{indent}{label}: (none — follows the launching session)");
        return;
    }
    cli_println!("{indent}{label}:");
    let rows = [
        ("tty", &t.tty),
        ("DISPLAY", &t.display),
        ("WAYLAND_DISPLAY", &t.wayland_display),
        ("XAUTHORITY", &t.xauthority),
        ("XDG_RUNTIME_DIR", &t.xdg_runtime_dir),
    ];
    for (name, value) in rows {
        if let Some(v) = value {
            cli_println!("{indent}  {name:<16}: {v}");
        }
    }
}

/// `config display [--tty …] [--clear]`.
pub(crate) async fn cmd_config_display(args: DisplayArgs, clear: bool, json: bool) -> Result<()> {
    let mut config = load_launcher_config().await?;
    let changed = clear || !args.is_empty();
    if clear {
        config.display = Default::default();
    } else {
        args.apply_to(&mut config.display);
    }
    // Fail early on a bad tty.
    let effective = aurelia::core::session_env::resolve_for_launch(&config.display, None);
    if changed {
        if let Err(e) = &effective {
            anyhow::bail!("not saved: {e:#}");
        }
        save_launcher_config(&config)
            .await
            .context("failed saving display config")?;
    }
    if json {
        print_json(&serde_json::json!({
            "display": config.display,
            "effective": effective.as_ref().ok().map(|e| e.vars().clone()),
            "error": effective.as_ref().err().map(|e| format!("{e:#}")),
        }));
        return Ok(());
    }
    print_display_target("", "Display override", &config.display);
    match &effective {
        Ok(env) => cli_println!("Launches use: {}", env.describe()),
        Err(e) => cli_println!("⚠ {e:#}"),
    }
    if changed {
        cli_println!("Saved. Applies to the next launch.");
    }
    Ok(())
}

/// `config wayland-native [on|off]`.
pub(crate) async fn cmd_config_wayland_native(mode: Option<OnOffArg>, json: bool) -> Result<()> {
    let (config, changed) = view_or_set(mode, |c, m| {
        c.wayland_native = matches!(m, OnOffArg::On);
    })
    .await?;
    let overrides: std::collections::BTreeMap<u32, bool> = config
        .game_configs
        .iter()
        .filter_map(|(id, g)| g.wayland_native.map(|on| (*id, on)))
        .collect();
    if json {
        print_json(&serde_json::json!({
            "wayland_native": config.wayland_native,
            "game_overrides": overrides,
        }));
        return Ok(());
    }
    let label = if config.wayland_native {
        "on (Proton with winewayland and SDL games use Wayland directly)"
    } else {
        "off (games use X11 via Xwayland)"
    };
    cli_println!("Native Wayland: {label}");
    for (id, on) in &overrides {
        cli_println!("  App {id}: {} (per-game)", if *on { "on" } else { "off" });
    }
    if changed {
        cli_println!("Saved. Applies to the next launch.");
    }
    Ok(())
}

/// `config anticheat-auto-install [on|off]`.
pub(crate) async fn cmd_config_anticheat_auto_install(
    mode: Option<OnOffArg>,
    json: bool,
) -> Result<()> {
    let (config, changed) = view_or_set(mode, |c, m| {
        c.anticheat_auto_install = matches!(m, OnOffArg::On);
    })
    .await?;
    if json {
        print_json(&serde_json::json!({
            "anticheat_auto_install": config.anticheat_auto_install
        }));
        return Ok(());
    }
    let label = if config.anticheat_auto_install {
        "on (`play` installs a missing EAC/BattlEye runtime before a Proton launch)"
    } else {
        "off (install runtimes with `aurelia proton anticheat eac|battleye`)"
    };
    cli_println!("Anti-cheat runtime auto-install: {label}");
    if changed {
        cli_println!("Saved. Applies to the next launch.");
    }
    Ok(())
}

/// Human form of a kbit/s limit.
pub(crate) fn describe_download_limit(kbit: u64) -> String {
    if kbit == 0 {
        return "unlimited".to_string();
    }
    let bytes = aurelia::core::config::kbit_to_bytes(kbit);
    let rate = if bytes >= 1_000_000 {
        format!("{:.1} MB/s", bytes as f64 / 1_000_000.0)
    } else {
        format!("{:.1} kB/s", bytes as f64 / 1_000.0)
    };
    format!("{kbit} kbit/s ({rate})")
}

/// `config download-limit [<kbit/s>|off]`.
pub(crate) async fn cmd_config_download_limit(kbit: Option<u64>, json: bool) -> Result<()> {
    let (config, changed) = view_or_set(kbit, |c, k| c.download_limit_kbit = k).await?;
    let current = config.download_limit_kbit;
    if json {
        print_json(&serde_json::json!({ "download_limit_kbit": current }));
        return Ok(());
    }
    cli_println!("Download limit: {}", describe_download_limit(current));
    if changed {
        cli_println!("Saved. Running downloads follow within a few seconds.");
    }
    Ok(())
}

/// `config install-resume [manual|auto]`.
pub(crate) async fn cmd_config_install_resume(
    mode: Option<InstallResumeArg>,
    json: bool,
) -> Result<()> {
    use aurelia::library::backlog::InstallResume;
    let (config, changed) = view_or_set(mode, |c, m| c.install_resume = m.into()).await?;
    if json {
        print_json(&serde_json::json!({ "install_resume": config.install_resume }));
        return Ok(());
    }
    let label = match config.install_resume {
        InstallResume::Manual => "manual (backlog installs wait for `aurelia install resume`)",
        InstallResume::Auto => {
            "auto (the daemon resumes interrupted installs when it starts; paused and stopped ones wait)"
        }
    };
    cli_println!("Install resume: {label}");
    if changed {
        cli_println!("Saved. Applies the next time the daemon starts.");
    }
    Ok(())
}

pub(crate) async fn cmd_config_steam_runtime_policy(
    policy: Option<SteamRuntimeArg>,
    json: bool,
) -> Result<()> {
    use aurelia::core::models::SteamRuntimePolicy;

    let (config, changed) =
        view_or_set(policy, |c, arg| c.steam_runtime_policy = arg.into()).await?;

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
        if config.steam_runtime_policy == SteamRuntimePolicy::Enabled
            && !aurelia::core::runner::STEAM_RUNTIME_SUPPORTED
        {
            cli_eprintln!(
                "Note: {}",
                aurelia::core::runner::steam_runtime_unsupported_msg()
            );
        }
    }
    Ok(())
}

/// `config protons`: list the Proton/Wine runtimes actually installed on disk.
/// Shares discovery with `proton list --installed` (no hardcoded placeholders).
pub(crate) async fn cmd_config_protons(json: bool) -> Result<()> {
    let cfg = load_launcher_config().await?;
    let installed =
        aurelia::compat::proton::list_installed(std::path::Path::new(&cfg.steam_library_path));
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
        cli_println!(
            "Install one with `aurelia proton install <NAME>` (see `aurelia proton list`)."
        );
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
    crate::app::commands::common::confirm_write(
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
    crate::app::commands::library::detect_installed_platform_set(
        &info.install_path.to_string_lossy(),
    )
}

/// Absolute, tidied form of a `--prefix` dir.
fn absolute_prefix_dir(dir: std::path::PathBuf) -> Result<std::path::PathBuf> {
    use std::path::Component;
    if dir.as_os_str().is_empty() {
        anyhow::bail!("--prefix needs a directory");
    }
    // Client cwd when run via daemon.
    let full = crate::app::output::user_path(&dir);
    let mut out = std::path::PathBuf::new();
    for part in full.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    if !out.is_absolute() {
        anyhow::bail!("--prefix needs an absolute directory");
    }
    Ok(out)
}

/// Drop all-default entries; report real changes.
fn settle_entry<T: Default + PartialEq>(
    store: &mut std::collections::HashMap<u32, T>,
    app_id: u32,
    before: Option<T>,
) -> bool {
    if store.get(&app_id).is_some_and(|e| *e == T::default()) {
        store.remove(&app_id);
    }
    store.get(&app_id) != before.as_ref()
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
        launch_options,
        no_launch_options,
        cloud_sync,
        prefix,
        no_prefix,
        display_target,
        no_display,
        wayland_native,
    } = args;
    let prefix = prefix.map(absolute_prefix_dir).transpose()?;
    if let Some(line) = &launch_options {
        aurelia::launch::stages::user_launch_options::parse_launch_options(line)
            .map_err(|e| anyhow::anyhow!("invalid --launch-options {line:?}: {e}"))?;
    }
    use aurelia::core::config::GameRunner;
    use aurelia::core::models::{SteamPrefixMode, SteamRuntimePolicy};

    // Saved anyway (a shared config may also be used on Linux), but say so.
    if !json && !aurelia::core::runner::PROTON_SUPPORTED {
        let linux_only: Vec<&str> = [
            (proton.is_some(), "--proton"),
            (
                matches!(platform, Some(PlatformArg::Linux)),
                "--platform linux",
            ),
            (native_engine, "--native-engine"),
            (umu, "--umu"),
            (
                matches!(steam_runtime, Some(SteamRuntimeArg::On)),
                "--steam-runtime on",
            ),
            (steam_prefix_mode.is_some(), "--steam-prefix-mode"),
            (prefix.is_some(), "--prefix"),
        ]
        .into_iter()
        .filter_map(|(used, flag)| used.then_some(flag))
        .collect();
        if !linux_only.is_empty() {
            cli_eprintln!(
                "Note: saved {}, but these only take effect on Linux, not on {}.",
                linux_only.join(", "),
                std::env::consts::OS
            );
        }
    }

    // The Steam-runtime knobs live in a separate per-game store (user_apps.json) from the
    // GameConfig fields above (config.json). Update whichever store each flag targets.
    let mut user_configs = aurelia::core::config::load_user_configs().await?;
    let user_before = user_configs.get(&app_id).cloned();
    if clear {
        user_configs.remove(&app_id);
    } else {
        let ua = user_configs.entry(app_id).or_default();
        if let Some(sr) = steam_runtime {
            ua.steam_runtime_policy = sr.into();
        }
        if let Some(pm) = steam_prefix_mode {
            ua.steam_prefix_mode = pm.into();
        }
        if no_launch_options {
            ua.launch_options.clear();
        } else if let Some(line) = launch_options {
            ua.launch_options = line.trim().to_string();
        }
    }
    let user_changed = settle_entry(&mut user_configs, app_id, user_before);
    if user_changed {
        aurelia::core::config::save_user_configs(&user_configs)
            .await
            .context("failed saving per-game config")?;
    }

    let mut cfg = load_launcher_config().await?;
    let before = cfg.game_configs.get(&app_id).cloned();
    if clear {
        cfg.game_configs.remove(&app_id);
    } else {
        let entry = cfg.game_configs.entry(app_id).or_default();
        if clear_proton {
            entry.forced_proton_version = None;
        } else if let Some(p) = proton {
            entry.forced_proton_version = Some(p);
        }
        if no_platform {
            entry.platform_preference = None;
        } else if let Some(pl) = platform {
            entry.platform_preference = Some(
                match pl {
                    PlatformArg::Windows => "windows",
                    PlatformArg::Linux => "linux",
                }
                .to_string(),
            );
        }
        if native_engine {
            entry.runner = GameRunner::Luxtorpeda;
        } else if no_native_engine {
            entry.runner = GameRunner::Auto;
        } else if umu {
            entry.runner = GameRunner::Umu;
        } else if no_umu {
            entry.runner = GameRunner::Auto;
        }
        if no_launch_script {
            entry.launch_script = None;
        } else if let Some(s) = launch_script {
            entry.launch_script = Some(s.to_string_lossy().to_string());
        }
        if let Some(mode) = cloud_sync {
            entry.cloud_sync = mode.as_override();
        }
        if no_prefix {
            entry.prefix_dir = None;
        } else if let Some(dir) = prefix {
            entry.prefix_dir = Some(dir);
        }
        if no_display {
            entry.display = Default::default();
        } else {
            display_target.apply_to(&mut entry.display);
        }
        if let Some(mode) = wayland_native {
            entry.wayland_native = mode.as_override();
        }
    }
    let changed = settle_entry(&mut cfg.game_configs, app_id, before);
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
    let cloud_sync_effective = cfg.cloud_sync_enabled(app_id);
    let on_off = |on: bool| if on { "on" } else { "off" };
    let cloud_sync_label = match entry.cloud_sync {
        Some(on) => on_off(on).to_string(),
        None => format!("auto ({}, from global)", on_off(cloud_sync_effective)),
    };
    let wine_prefix = aurelia::core::paths::proton_compat_prefix(&cfg, app_id, None);
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
            "launch_options": ua.launch_options,
            "cloud_sync": entry.cloud_sync,
            "cloud_sync_effective": cloud_sync_effective,
            "prefix_dir": entry.prefix_dir,
            "wine_prefix": wine_prefix,
            "display": entry.display,
            "wayland_native": entry.wayland_native,
            "wayland_native_effective": cfg.wayland_native(app_id),
        }));
    } else {
        cli_println!("App {app_id}:");
        let proton_default = format!("(global default: {})", cfg.proton_version);
        cli_println!(
            "  Proton       : {}",
            entry
                .forced_proton_version
                .as_deref()
                .unwrap_or(&proton_default)
        );
        let platform_auto = format!("(auto: {platform_resolved})");
        cli_println!(
            "  Platform     : {}",
            entry
                .platform_preference
                .as_deref()
                .unwrap_or(&platform_auto)
        );
        cli_println!("  Runner       : {runner_label}");
        cli_println!(
            "  Script       : {}",
            entry
                .launch_script
                .as_deref()
                .unwrap_or("(auto-detected / none)")
        );
        cli_println!("  Steam runtime: {steam_runtime_label}");
        cli_println!("  Prefix mode  : {prefix_mode_label}");
        cli_println!("  Cloud sync   : {cloud_sync_label}");
        cli_println!(
            "  Wine prefix  : {}{}",
            wine_prefix.display(),
            if entry.prefix_dir.is_some() {
                " (custom)"
            } else {
                ""
            }
        );
        cli_println!(
            "  Launch opts  : {}",
            if ua.launch_options.is_empty() {
                "(none)"
            } else {
                &ua.launch_options
            }
        );
        let wayland_label = match entry.wayland_native {
            Some(on) => on_off(on).to_string(),
            None => format!("auto ({}, from global)", on_off(cfg.wayland_native)),
        };
        cli_println!("  Wayland      : {wayland_label}");
        if !entry.display.is_empty() {
            print_display_target("  ", "Display", &entry.display);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
