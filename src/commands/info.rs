//! `info` command handlers.

use crate::steam_urls;

use crate::commands::common::*;

use std::path::PathBuf;
use anyhow::{bail, Context, Result};
use aurelia::core::config::info_cache_ttl;
use aurelia::core::config::load_info_cache;
use aurelia::core::config::save_info_cache;
use aurelia::steam_client::{SteamClient, StoreAppInfo};

pub(crate) async fn cmd_set_dlc(app_id: u32, enable: bool, restart_steam: bool, json: bool) -> Result<()> {
    let client = restored_client().await?;

    // Steam flushes its in-memory app state on exit, so the edit must happen while
    // Steam is stopped to survive. With --restart-steam: stop → edit → start.
    let manage_steam = restart_steam && SteamClient::steam_is_running();
    if manage_steam {
        if !json {
            cli_println!("Stopping Steam ...");
        }
        SteamClient::shutdown_steam()?;
    }

    let base = client
        .set_dlc_enabled(app_id, enable)
        .await
        .with_context(|| {
            format!(
                "failed to {} DLC {app_id}",
                if enable { "enable" } else { "disable" }
            )
        })?;

    let mut steam_restarted = false;
    if manage_steam {
        if !json {
            cli_println!("Starting Steam ...");
        }
        SteamClient::start_steam()?;
        steam_restarted = true;
    }

    let action = if enable { "enabled" } else { "disabled" };
    let restart_required = !manage_steam && SteamClient::steam_is_running();

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "base_app_id": base,
            "status": action,
            "steam_restarted": steam_restarted,
            "steam_restart_required": restart_required,
        }));
        return Ok(());
    }

    cli_println!("DLC {app_id} {action} for base game {base}.");
    if enable {
        cli_println!("(Toggles the flag only — run `aurelia install {app_id}` if the content isn't downloaded.)");
    }
    if restart_required {
        cli_eprintln!("Note: Steam is running and reads DLC state only at startup, so this won't apply");
        cli_eprintln!("      until you restart Steam (or re-run with --restart-steam).");
    }
    Ok(())
}

pub(crate) async fn cmd_branches(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let branches = client
        .fetch_branches(app_id)
        .await
        .with_context(|| format!("failed to fetch branches for app {app_id}"))?;
    if json {
        print_json(&serde_json::json!({ "app_id": app_id, "branches": branches }));
        return Ok(());
    }
    if branches.is_empty() {
        cli_println!("No branches reported.");
    } else {
        for b in branches {
            cli_println!("{b}");
        }
    }
    Ok(())
}

pub(crate) async fn cmd_set_branch(app_id: u32, branch: String, json: bool) -> Result<()> {
    let client = authed_client().await?;
    client
        .update_app_branch(app_id, &branch)
        .await
        .with_context(|| format!("failed to switch app {app_id} to branch {branch}"))?;
    if json {
        print_json(&serde_json::json!({ "app_id": app_id, "branch": branch, "status": "set" }));
    } else {
        cli_println!("App {app_id} set to branch '{branch}'. Run `aurelia update {app_id}` to apply.");
    }
    Ok(())
}

/// Storefront-only `--extended` data for one app: the HTTPS `AppDetails` plus the
/// SteamSpy user tags.
pub(crate) type ExtendedInfo = (aurelia::web::store::AppDetails, Vec<String>);

pub(crate) async fn cmd_info(
    app_ids: Vec<u32>,
    extended: bool,
    no_cache: bool,
    lang: Option<String>,
    json: bool,
) -> Result<()> {
    // Resolve the store-text language once: explicit --lang > `config language` >
    // English. Threaded into the StoreBrowse fetch, the `--extended` storefront
    // fetch, and the per-language cache key.
    let lang = resolve_steam_language(lang).await;
    // The CM-sourced metadata (StoreBrowse + the DLC list) is effectively static
    // for hours, and drivers like Heroic call `info` repeatedly. Serve it from a
    // short-TTL disk cache so a repeat call avoids the Steam CM logon and the
    // StoreBrowse/PICS round-trips entirely. `--no-cache` forces a fresh fetch.
    let ttl = if no_cache {
        std::time::Duration::ZERO
    } else {
        info_cache_ttl()
    };
    let single = app_ids.len() == 1;

    // Partition the requested ids into cache hits and misses. Every miss is then
    // fetched over a *single* Steam logon with one batched StoreBrowse call, so
    // `info 1 2 3` costs one logon — not the three a caller spends running `info 1`,
    // `info 2`, `info 3` separately.
    let mut base: std::collections::HashMap<u32, (StoreAppInfo, Vec<(u32, Option<String>)>)> =
        std::collections::HashMap::new();
    let mut misses: Vec<u32> = Vec::new();
    for &id in &app_ids {
        match load_info_cache(id, &lang, ttl).await {
            Some(cached) => {
                base.insert(id, (cached.details, cached.dlc));
            }
            None => misses.push(id),
        }
    }

    if !misses.is_empty() {
        // Metadata comes from the StoreBrowse service over the Steam CM connection
        // (no HTTPS storefront API), so a session is needed here.
        let client = authed_client().await?;
        let store = client
            .fetch_store_apps(&misses, &lang)
            .await
            .context("failed to fetch store information")?;
        for &id in &misses {
            let Some(details) = store.iter().find(|i| i.app_id == id).cloned() else {
                // A single-id caller expects a hard error for an unknown app; in a
                // batch we skip it (with a warning) so one delisted id doesn't sink
                // the rest.
                if single {
                    bail!("no store information available for app {id}");
                }
                tracing::warn!("no store information available for app {id}; skipping");
                continue;
            };

            // The DLC id list isn't part of StoreBrowse's per-item data; read it
            // from PICS appinfo (`common.dlc`), then resolve the DLC names in a
            // single batched StoreBrowse call.
            let dlc_ids = client
                .get_extended_app_info(id)
                .await
                .map(|e| e.dlcs)
                .unwrap_or_default();
            let dlc = resolve_dlc_names_via_store(&client, &dlc_ids, &lang).await;

            // Best-effort cache write — a failure here must not fail the command.
            if let Err(e) = save_info_cache(id, &lang, &details, &dlc).await {
                tracing::warn!("could not cache info for app {id}: {e:#}");
            }
            base.insert(id, (details, dlc));
        }
    }

    // Storefront-only fields (system requirements, Metacritic, website, store
    // genres/categories, SteamSpy user tags). These have no CM-protocol source, so
    // `--extended` fetches them from the public HTTPS storefront, reusing one HTTP
    // client across ids. Best-effort: any failure leaves them absent.
    let mut extended_by_id: std::collections::HashMap<u32, ExtendedInfo> =
        std::collections::HashMap::new();
    if extended {
        match reqwest::Client::builder().user_agent("aurelia/0.1").build() {
            Ok(http) => {
                for &id in &app_ids {
                    if !base.contains_key(&id) {
                        continue;
                    }
                    let web = aurelia::web::store::fetch_app_details(&http, id, &lang).await.ok().flatten();
                    let tags = aurelia::web::store::fetch_tags(&http, id).await;
                    if let Some(d) = web {
                        extended_by_id.insert(id, (d, tags));
                    }
                }
            }
            Err(e) => tracing::warn!("could not build HTTP client for --extended: {e:#}"),
        }
    }

    // --- Render in the order the ids were requested ---
    if json {
        let items: Vec<serde_json::Value> = app_ids
            .iter()
            .filter_map(|id| {
                base.get(id)
                    .map(|(details, dlc)| info_json_value(details, dlc, extended_by_id.get(id)))
            })
            .collect();
        // One id keeps the original single-object shape (backward compatible);
        // several ids produce an array.
        if single {
            match items.into_iter().next() {
                Some(v) => cli_println!("{}", serde_json::to_string_pretty(&v)?),
                None => bail!("no store information available for app {}", app_ids[0]),
            }
        } else {
            cli_println!("{}", serde_json::to_string_pretty(&serde_json::Value::Array(items))?);
        }
        return Ok(());
    }

    let mut first = true;
    for id in &app_ids {
        let Some((details, dlc)) = base.get(id) else {
            continue;
        };
        if !first {
            cli_println!("\n{}", "─".repeat(60));
        }
        first = false;
        print_info_human(details, dlc, extended_by_id.get(id));
    }
    Ok(())
}

/// Build the `--json` object for one app from its CM metadata, DLC list and
/// optional `--extended` storefront data.
pub(crate) fn info_json_value(
    details: &StoreAppInfo,
    dlc: &[(u32, Option<String>)],
    extended_info: Option<&ExtendedInfo>,
) -> serde_json::Value {
    let mut value = serde_json::json!({
        "app_id": details.app_id,
        "name": details.name,
        "type": details.app_type,
        "is_free": details.is_free,
        "early_access": details.is_early_access,
        "description": details.short_description,
        "full_description": details.full_description,
        "developers": details.developers,
        "publishers": details.publishers,
        "franchises": details.franchises,
        "release_date": details.release_date,
        "coming_soon": details.coming_soon,
        "price": details.price,
        "discount_pct": details.discount_pct,
        "platforms": details.platforms,
        "reviews": details.review_summary,
        "store_url": steam_urls::store_url(details.app_id),
        "assets": {
            "header": details.assets.header,
            "capsule": details.assets.capsule,
            "hero": details.assets.hero,
            "background": details.assets.background,
            "logo": details.assets.logo,
        },
        "dlc": dlc.iter().map(|(id, name)| serde_json::json!({"app_id": id, "name": name})).collect::<Vec<_>>(),
    });
    if let Some((web, tags)) = extended_info {
        value["extended"] = serde_json::json!({
            "genres": web.genres,
            "categories": web.categories,
            "tags": tags,
            "metacritic": web.metacritic,
            "website": web.website,
            "requirements": {
                "minimum": web.requirements_minimum,
                "recommended": web.requirements_recommended,
            },
        });
    }
    value
}

/// Render the human-readable `info` block for one app to stdout.
pub(crate) fn print_info_human(
    details: &StoreAppInfo,
    dlc: &[(u32, Option<String>)],
    extended_info: Option<&ExtendedInfo>,
) {
    // --- Header ---
    let ea = if details.is_early_access { " [Early Access]" } else { "" };
    cli_println!("{}  (app {}){ea}", details.name, details.app_id);
    if !details.app_type.is_empty() {
        cli_println!("Type       : {}", details.app_type);
    }
    if !details.developers.is_empty() {
        cli_println!("Developers : {}", details.developers.join(", "));
    }
    if !details.publishers.is_empty() {
        cli_println!("Publishers : {}", details.publishers.join(", "));
    }
    if !details.franchises.is_empty() {
        cli_println!("Franchises : {}", details.franchises.join(", "));
    }
    if let Some(date) = &details.release_date {
        let suffix = if details.coming_soon { " (coming soon)" } else { "" };
        cli_println!("Released   : {date}{suffix}");
    }
    if let Some(price) = &details.price {
        let discount = if details.discount_pct > 0 {
            format!(" (-{}%)", details.discount_pct)
        } else {
            String::new()
        };
        cli_println!("Price      : {price}{discount}");
    }
    if !details.platforms.is_empty() {
        cli_println!("Platforms  : {}", details.platforms.join(", "));
    }
    if let Some(reviews) = &details.review_summary {
        cli_println!("Reviews    : {reviews}");
    }
    if let Some((web, _)) = extended_info {
        if let Some(score) = web.metacritic {
            cli_println!("Metacritic : {score}");
        }
        if let Some(site) = &web.website {
            cli_println!("Website    : {site}");
        }
    }

    // --- Description ---
    if !details.short_description.is_empty() {
        cli_println!("\nDescription:");
        for line in wrap_text(&details.short_description, 88) {
            cli_println!("  {line}");
        }
    }

    // --- Extended: tags / genres / categories / requirements ---
    if let Some((web, tags)) = extended_info {
        if !tags.is_empty() {
            cli_println!(
                "\nTags      : {}",
                tags.iter().take(20).cloned().collect::<Vec<_>>().join(", ")
            );
        }
        if !web.genres.is_empty() {
            cli_println!("Genres    : {}", web.genres.join(", "));
        }
        if !web.categories.is_empty() {
            cli_println!("Categories: {}", web.categories.join(", "));
        }
        if !web.requirements_minimum.is_empty() {
            cli_println!("\nMinimum requirements:");
            for line in &web.requirements_minimum {
                cli_println!("  {line}");
            }
        }
        if !web.requirements_recommended.is_empty() {
            cli_println!("\nRecommended requirements:");
            for line in &web.requirements_recommended {
                cli_println!("  {line}");
            }
        }
    }

    // --- Artwork ---
    let a = &details.assets;
    if a.header.is_some() || a.capsule.is_some() || a.background.is_some() {
        cli_println!("\nArtwork:");
        if let Some(u) = &a.header {
            cli_println!("  header    : {u}");
        }
        if let Some(u) = &a.capsule {
            cli_println!("  capsule   : {u}");
        }
        if let Some(u) = &a.hero {
            cli_println!("  hero      : {u}");
        }
        if let Some(u) = &a.background {
            cli_println!("  background: {u}");
        }
        if let Some(u) = &a.logo {
            cli_println!("  logo      : {u}");
        }
    }

    // --- DLC ---
    if !dlc.is_empty() {
        cli_println!("\nDLC ({}):", dlc.len());
        for (id, name) in dlc {
            let name = name.clone().unwrap_or_else(|| "(name unavailable)".to_string());
            cli_println!("  {id:>9}  {name}");
        }
    }
}

/// Resolve DLC names via a single batched `StoreBrowse.GetItems` call (over the
/// Steam CM connection — no storefront API), returning `(app_id, name)` pairs
/// sorted by app id. A `None` name means the store didn't return that id.
pub(crate) async fn resolve_dlc_names_via_store(
    client: &SteamClient,
    dlc_ids: &[u32],
    language: &str,
) -> Vec<(u32, Option<String>)> {
    if dlc_ids.is_empty() {
        return Vec::new();
    }
    let name_by_id: std::collections::HashMap<u32, String> = client
        .fetch_store_apps(dlc_ids, language)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|i| (i.app_id, i.name))
        .collect();

    let mut dlc: Vec<(u32, Option<String>)> = dlc_ids
        .iter()
        .map(|&id| (id, name_by_id.get(&id).cloned().filter(|s| !s.is_empty())))
        .collect();
    dlc.sort_by_key(|(id, _)| *id);
    dlc
}

pub(crate) async fn cmd_dlc(app_id: u32, json: bool) -> Result<()> {
    // Ownership status requires an authenticated connection; installed/disabled status
    // is read from the local appmanifest.
    let steam = authed_client().await?;

    // DLC ids come from PICS appinfo (`common.dlc`); names from a batched
    // StoreBrowse call — both over the Steam CM connection, no storefront API.
    let dlc_ids: Vec<u32> = steam
        .get_extended_app_info(app_id)
        .await
        .map(|e| e.dlcs)
        .unwrap_or_default();
    let dlc = resolve_dlc_names_via_store(&steam, &dlc_ids, "english").await;
    let states = steam
        .dlc_states(app_id, &dlc_ids)
        .await
        .with_context(|| format!("failed to resolve DLC status for app {app_id}"))?;
    let state_by_id: std::collections::HashMap<u32, &aurelia::core::models::DlcState> =
        states.iter().map(|s| (s.app_id, s)).collect();

    if json {
        let value = serde_json::json!({
            "app_id": app_id,
            "dlc": dlc.iter().map(|(id, name)| {
                let s = state_by_id.get(id);
                serde_json::json!({
                    "app_id": id,
                    "name": name,
                    "owned": s.map(|s| s.owned),
                    "installed": s.map(|s| s.installed),
                    "disabled": s.map(|s| s.disabled),
                    "image_url": steam_urls::header_url(*id),
                    "image_fallback_url": steam_urls::small_capsule_url(*id),
                    "store_url": steam_urls::store_url(*id),
                })
            }).collect::<Vec<_>>(),
        });
        cli_println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    if dlc.is_empty() {
        cli_println!("No DLC for app {app_id}.");
        return Ok(());
    }
    cli_println!("{:>9}  {:<5}  {:<13}  NAME", "APPID", "OWNED", "STATUS");
    for (id, name) in &dlc {
        let name = name.clone().unwrap_or_else(|| "(name unavailable)".to_string());
        let s = state_by_id.get(id);
        let owned = match s.map(|s| s.owned) {
            Some(true) => "yes",
            Some(false) => "no",
            None => "?",
        };
        // Installed/disabled describe the local content state of an owned DLC.
        let status = match s {
            Some(s) if !s.installed => "not-installed",
            Some(s) if s.disabled => "disabled",
            Some(_) => "enabled",
            None => "?",
        };
        cli_println!("{id:>9}  {owned:<5}  {status:<13}  {name}");
    }
    Ok(())
}

pub(crate) async fn cmd_achievements(app_id: u32, lang: Option<String>, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let lang = resolve_steam_language(lang).await;
    let achievements = client
        .fetch_achievements(app_id, &lang)
        .await
        .with_context(|| format!("failed to fetch achievements for app {app_id}"))?;
    let unlocked = achievements.iter().filter(|a| a.unlocked).count();

    if json {
        let arr: Vec<_> = achievements
            .iter()
            .map(|a| {
                serde_json::json!({
                    "achievement_id": a.api_name,
                    "achievement_key": a.api_name,
                    "name": a.name,
                    "description": a.description,
                    "visible": !a.hidden,
                    "image_url_unlocked": a.icon_unlocked,
                    "image_url_locked": a.icon_locked,
                    "rarity": a.global_percent,
                    "unlocked": a.unlocked,
                    "unlock_time": a.unlock_time,
                    "date_unlocked": a.unlock_time.map(|t| format_unix_timestamp(u64::from(t))),
                })
            })
            .collect();
        print_json(&serde_json::json!({
            "app_id": app_id,
            "unlocked": unlocked,
            "total": achievements.len(),
            "achievements": arr,
        }));
        return Ok(());
    }

    if achievements.is_empty() {
        cli_println!("No achievements for app {app_id}.");
        return Ok(());
    }
    cli_println!("{:<2}  {:>6}  {:<19}  NAME", "", "RARITY", "UNLOCKED");
    for a in &achievements {
        let mark = if a.unlocked { "✓" } else { " " };
        let when = a
            .unlock_time
            .map(|t| format_unix_timestamp(u64::from(t)))
            .unwrap_or_else(|| "-".to_string());
        let name = if a.hidden && !a.unlocked {
            format!("{} (hidden)", a.name)
        } else {
            a.name.clone()
        };
        cli_println!("{mark:<2}  {:>5.1}%  {when:<19}  {name}", a.global_percent);
    }
    cli_println!("\n{unlocked}/{} unlocked.", achievements.len());
    Ok(())
}

pub(crate) async fn cmd_depots(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let depots = client
        .get_depot_list(app_id)
        .await
        .with_context(|| format!("failed to load depots for app {app_id}"))?;

    if json {
        let arr: Vec<_> = depots
            .iter()
            .map(|d| {
                serde_json::json!({
                    "id": d.id,
                    "name": d.name,
                    "size": d.size,
                    "file_count": d.file_count,
                    "config": d.config,
                    "is_owned": d.is_owned,
                })
            })
            .collect();
        print_json(&serde_json::json!({ "app_id": app_id, "depots": arr }));
        return Ok(());
    }

    if depots.is_empty() {
        cli_println!("No depots reported.");
        return Ok(());
    }
    cli_println!("{:>12}  {:>14}  NAME", "DEPOT", "SIZE(bytes)");
    for d in &depots {
        cli_println!("{:>12}  {:>14}  {}", d.id, d.size, d.name);
    }
    Ok(())
}

pub(crate) async fn cmd_launch_options(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let options = client
        .fetch_launch_options(app_id)
        .await
        .with_context(|| format!("failed to load launch options for app {app_id}"))?;

    if json {
        let arr: Vec<_> = options
            .iter()
            .map(|o| {
                serde_json::json!({
                    "id": o.id,
                    "description": o.description,
                    "executable": o.executable,
                    "arguments": o.arguments,
                    "working_dir": o.working_dir,
                    "oslist": o.oslist,
                    "osarch": o.osarch,
                    "type": o.launch_type,
                })
            })
            .collect();
        print_json(&serde_json::json!({ "app_id": app_id, "launch_options": arr }));
        return Ok(());
    }

    if options.is_empty() {
        cli_println!("No launch options for app {app_id}.");
        return Ok(());
    }
    cli_println!("{:>3}  {:<10}  NAME / COMMAND", "ID", "OS");
    for o in &options {
        let os = if o.oslist.is_empty() { "any" } else { &o.oslist };
        let desc = if o.description.is_empty() {
            &o.executable
        } else {
            &o.description
        };
        cli_println!("{:>3}  {:<10}  {}", o.id, os, desc);
        let cmd = format!("{} {}", o.executable, o.arguments);
        let cmd = cmd.trim();
        if !cmd.is_empty() {
            cli_println!("       {cmd}");
        }
    }
    Ok(())
}

pub(crate) async fn cmd_image(app_id: u32, output: Option<PathBuf>, force: bool, json: bool) -> Result<()> {
    let cache_dir = aurelia::core::config::opensteam_image_cache_dir()?;
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .with_context(|| format!("failed creating {}", cache_dir.display()))?;
    let cache_path = cache_dir.join(format!("{app_id}_library.jpg"));

    if force || tokio::fs::metadata(&cache_path).await.is_err() {
        // Steam CDN artwork variants, tried in order of preference.
        let candidates = [
            format!("https://cdn.akamai.steamstatic.com/steam/apps/{app_id}/library_600x900_2x.jpg"),
            format!("https://cdn.akamai.steamstatic.com/steam/apps/{app_id}/header.jpg"),
            format!("https://steamcdn-a.akamaihd.net/steam/apps/{app_id}/library_capsule_2x.jpg"),
        ];

        let mut downloaded = false;
        for url in candidates {
            match reqwest::get(&url).await {
                Ok(resp) if resp.status().is_success() => {
                    let bytes = resp.bytes().await.context("failed reading image bytes")?;
                    tokio::fs::write(&cache_path, &bytes)
                        .await
                        .with_context(|| format!("failed writing {}", cache_path.display()))?;
                    downloaded = true;
                    break;
                }
                _ => continue,
            }
        }

        if !downloaded {
            bail!("no artwork found on the Steam CDN for app {app_id}");
        }
    }

    let final_path = match output {
        Some(out) => {
            tokio::fs::copy(&cache_path, &out)
                .await
                .with_context(|| format!("failed writing {}", out.display()))?;
            out
        }
        None => cache_path,
    };
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "path": final_path.display().to_string(),
        }));
    } else {
        cli_println!("{}", final_path.display());
    }
    Ok(())
}
