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
        aurelia::core::utils::resolve_steam_runtime_wine(&current, &library_root).ok()
    } else {
        None
    };

    if json {
        print_json(&serde_json::json!({
            "steam_runtime_runner": configured.then(|| current.clone()),
            "resolved_wine": resolved.as_ref().map(|p: &PathBuf| p.display().to_string()),
            "steam_runtime_supported": aurelia::core::utils::STEAM_RUNTIME_SUPPORTED,
        }));
        return Ok(());
    }
    if !aurelia::core::utils::STEAM_RUNTIME_SUPPORTED {
        cli_eprintln!(
            "Note: {}",
            aurelia::core::utils::steam_runtime_unsupported_msg()
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
/// Steam runtime). See [`crate::cli::ConfigCommand::SteamRuntimePolicy`].
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
    let cloud = if config.steamworks_cloud_write {
        "games may write"
    } else {
        "read-only"
    };
    cli_println!("Steam Cloud for stand-in games: {cloud}");
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

/// `config steamworks-client` arguments.
pub(crate) struct SteamworksClientArgs {
    pub mode: Option<StandInModeArg>,
    pub path: Option<String>,
    pub stats_writes: Option<OnOffArg>,
    pub allow: Option<u32>,
    pub disallow: Option<u32>,
    pub cloud_write: Option<OnOffArg>,
    pub workshop_publish: Option<OnOffArg>,
    pub steam_install: Option<StandInInstallArg>,
    pub yes: bool,
}

/// Warning before enabling Cloud writes.
pub(crate) fn cloud_write_enable_prompt() -> String {
    "Let stand-in games write and delete their Steam Cloud files? Files they \
     write are uploaded by the Cloud sync after the game exits; a game that \
     writes a broken save spreads it to every device. [y/N] "
        .to_string()
}

/// `config steamworks-client [on|off] [--path]`.
pub(crate) async fn cmd_config_steamworks_client(
    args: SteamworksClientArgs,
    json: bool,
) -> Result<()> {
    if let Some(app_id) = args.allow {
        let prompt = aurelia::launch::stand_in_guard::allow_prompt(app_id);
        confirm_write("allow an anti-cheat game", &prompt, args.yes, json)?;
    }
    if matches!(args.cloud_write, Some(OnOffArg::On)) {
        let prompt = cloud_write_enable_prompt();
        confirm_write("enable Cloud writes", &prompt, args.yes, json)?;
    }
    let SteamworksClientArgs {
        mode,
        path,
        stats_writes,
        allow,
        disallow,
        cloud_write,
        workshop_publish,
        steam_install,
        ..
    } = args;
    let any = steam_install.is_some()
        || mode.is_some()
        || path.is_some()
        || stats_writes.is_some()
        || allow.is_some()
        || disallow.is_some()
        || cloud_write.is_some()
        || workshop_publish.is_some();
    // Install first; never save broken paths.
    let installed = match path.as_deref().map(str::trim) {
        Some(p) if !p.is_empty() => Some(install_stand_in(p)?),
        _ => None,
    };
    let reset_path = path.as_deref().is_some_and(|p| p.trim().is_empty());
    if matches!(mode, Some(StandInModeArg::On | StandInModeArg::Auto)) {
        ensure_stand_in_installed(installed.is_some() || reset_path).await?;
    }
    let path_change = installed.is_some() || reset_path;
    let change = any.then_some((
        mode,
        path_change,
        stats_writes,
        allow,
        disallow,
        cloud_write,
        (workshop_publish, steam_install),
    ));
    let (config, changed) = view_or_set(
        change,
        |c, (mode, path, writes, allow, disallow, cloud_write, (publish, install))| {
            if let Some(install) = install {
                c.steamworks_steam_install = install.into();
            }
            if let Some(mode) = mode {
                c.steamworks_client = matches!(mode, StandInModeArg::On);
                c.steamworks_client_auto = matches!(mode, StandInModeArg::Auto);
            }
            if let Some(writes) = writes {
                c.steamworks_stats_writes = matches!(writes, OnOffArg::On);
            }
            // Aurelia's own directory again.
            if path {
                c.steamworks_client_path = None;
            }
            let allowed = &mut c.steamworks_allowed_protected;
            if let Some(id) = allow.filter(|id| !allowed.contains(id)) {
                allowed.push(id);
                allowed.sort_unstable();
            }
            if let Some(id) = disallow {
                allowed.retain(|a| *a != id);
            }
            if let Some(cloud_write) = cloud_write {
                c.steamworks_cloud_write = matches!(cloud_write, OnOffArg::On);
            }
            if let Some(publish) = publish {
                c.steamworks_workshop_publish = matches!(publish, OnOffArg::On);
            }
        },
    )
    .await?;
    let default_dir = steamworks_default_dir();
    let effective_path = config
        .steamworks_client_path
        .clone()
        .or_else(|| default_dir.clone());
    if json {
        print_json(&serde_json::json!({
            "steamworks_client": config.steamworks_client,
            "steamworks_client_mode": stand_in_mode_name(&config),
            "steamworks_client_path": config.steamworks_client_path,
            "effective_path": effective_path,
            "installed": installed.as_ref().map(|i| serde_json::json!({
                "sdk64": i.sdk64,
                "sdk32": i.sdk32,
            })),
            "steamworks_allowed_protected": config.steamworks_allowed_protected,
            "steamworks_cloud_write": config.steamworks_cloud_write,
            "steamworks_stats_writes": config.steamworks_stats_writes,
            "steamworks_workshop_publish": config.steamworks_workshop_publish,
            "steamworks_steam_install": config.steamworks_steam_install,
        }));
        return Ok(());
    }
    let label = match stand_in_mode_name(&config) {
        "on" => "on (`play` loads the stand-in steamclient.so in a private namespace)",
        "auto" => "auto (stand-in only while Valve's Steam client is not running)",
        _ => "off",
    };
    cli_println!("Stand-in steamclient.so: {label}");
    cli_println!(
        "Steam install: {}",
        steam_install_label(config.steamworks_steam_install)
    );
    if let Some(i) = &installed {
        let show = |p: &Option<std::path::PathBuf>| {
            p.as_ref()
                .map_or("none".to_string(), |p| p.display().to_string())
        };
        cli_println!("Installed 64-bit: {}", show(&i.sdk64));
        cli_println!("Installed 32-bit: {}", show(&i.sdk32));
        cli_println!(
            "Checked: ELF architecture and {} export OK",
            aurelia::launch::stand_in_lib::ENTRY_SYMBOL
        );
        if stand_in_mode_name(&config) == "off" {
            cli_println!("Turn it on with `aurelia config steamworks-client on` (or `auto`).");
        }
    }
    if let Some(path) = effective_path {
        cli_println!("Path: {path}");
    }
    let writes = if config.steamworks_stats_writes {
        "on (games may store stats and unlock achievements)"
    } else {
        "off (games read stats only)"
    };
    cli_println!("Stats writes: {writes}");
    let publish = if config.steamworks_workshop_publish {
        "on (games may create and edit Workshop items)"
    } else {
        "off"
    };
    cli_println!("Workshop publishing: {publish}");
    if !config.steamworks_allowed_protected.is_empty() {
        let ids: Vec<String> = config
            .steamworks_allowed_protected
            .iter()
            .map(u32::to_string)
            .collect();
        cli_println!("Anti-cheat games allowed: {}", ids.join(", "));
    }
    if changed {
        cli_println!("Saved. Applies to the next launch.");
    }
    Ok(())
}

/// `host`, `in-wine` or `auto`.
pub(crate) fn steam_install_label(v: aurelia::core::models::StandInSteamInstall) -> &'static str {
    use aurelia::core::models::StandInSteamInstall as S;
    match v {
        S::Auto => "auto (in-Wine Steam if installed, else host Steam)",
        S::Host => "host (host Steam compatdata; Proton installs its steamclient files)",
        S::InWine => "in-wine (Aurelia's in-Wine Steam prefix, DLLs shadowed per launch)",
    }
}

/// `on`, `off` or `auto`.
pub(crate) fn stand_in_mode_name(config: &aurelia::core::config::LauncherConfig) -> &'static str {
    if config.steamworks_client {
        "on"
    } else if config.steamworks_client_auto {
        "auto"
    } else {
        "off"
    }
}

/// Resolves, checks, copies `input`.
fn install_stand_in(input: &str) -> Result<aurelia::launch::stand_in_lib::Resolved> {
    use aurelia::launch::stand_in_lib;
    let dir = steamworks_default_dir()
        .ok_or_else(|| anyhow::anyhow!("the stand-in steamclient.so is Linux only"))?;
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    let home = aurelia::core::config::home_dir()?;
    let found = stand_in_lib::resolve(input, &cwd, &home)?;
    stand_in_lib::install(&found, std::path::Path::new(&dir))
}

/// `on` needs an installed 64-bit library.
async fn ensure_stand_in_installed(default_dir: bool) -> Result<()> {
    let config = load_launcher_config().await?;
    let dir = match (&config.steamworks_client_path, default_dir) {
        (Some(custom), false) => Some(custom.clone()),
        _ => steamworks_default_dir(),
    }
    .ok_or_else(|| anyhow::anyhow!("the stand-in steamclient.so is Linux only"))?;
    if aurelia::launch::steamworks_overlay::resolve_client_libs(std::path::Path::new(&dir)).is_err()
    {
        anyhow::bail!(
            "no stand-in steamclient.so is installed in {dir}; build aurelia-steamworks and \
             run `aurelia config steamworks-client --path <libsteamclient.so or checkout>` first"
        );
    }
    Ok(())
}

/// Default stand-in directory, Linux only.
fn steamworks_default_dir() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        aurelia::launch::steamworks_overlay::default_client_dir()
            .map(|d| d.to_string_lossy().into_owned())
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
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
            && !aurelia::core::utils::STEAM_RUNTIME_SUPPORTED
        {
            cli_eprintln!(
                "Note: {}",
                aurelia::core::utils::steam_runtime_unsupported_msg()
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
        steam_install,
    } = args;
    use aurelia::core::config::GameRunner;
    use aurelia::core::models::{SteamPrefixMode, SteamRuntimePolicy};

    // Saved anyway (a shared config may also be used on Linux), but say so.
    if !json && !aurelia::core::utils::PROTON_SUPPORTED {
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
        if let Some(si) = steam_install {
            ua.stand_in_steam_install = Some(si.into());
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
            "stand_in_steam_install": ua.stand_in_steam_install,
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
        let install = ua
            .stand_in_steam_install
            .map_or("(global)", steam_install_label);
        cli_println!("  Steam install: {install}");
    }
    Ok(())
}
