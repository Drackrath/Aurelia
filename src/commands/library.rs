//! `library` command handlers.

use crate::steam_urls;

use crate::commands::common::*;

use anyhow::{bail, Context, Result};

/// Best-effort detection of the platform whose depot is installed for a game, by
/// looking for a Windows executable in its install directory: a Windows depot
/// always ships a `.exe`, a native Linux/macOS build never does. Breadth-first so
/// a Windows game's top-level `.exe` is found immediately; the walk is bounded so
/// a large native install can't stall `list`. Returns `None` when the directory
/// can't be read or the budget is exhausted before a verdict (the caller then
/// leaves the platform unknown rather than guessing).
pub(crate) fn detect_installed_platform(install_path: &str) -> Option<String> {
    let root = std::path::Path::new(install_path);
    if !root.is_dir() {
        return None;
    }
    let mut queue = std::collections::VecDeque::from([root.to_path_buf()]);
    let mut budget = 100_000usize;
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if budget == 0 {
                return None;
            }
            budget -= 1;
            let path = entry.path();
            if path.is_dir() {
                queue.push_back(path);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
            {
                return Some("windows".to_string());
            }
        }
    }
    Some("linux".to_string())
}

pub(crate) async fn cmd_list(
    installed: bool,
    search: Option<String>,
    collection: Option<String>,
    online: bool,
    check_updates: bool,
    json: bool,
) -> Result<()> {
    let mut client = restored_client().await?;
    let mut games = load_library(&mut client).await;

    // Load the local collections store (best-effort; ignore if none). Only static
    // collections contribute here — dynamic (filter-based) collections have no
    // explicit membership list, so their members can't be resolved offline.
    let collections_store = aurelia::library::collections::CollectionsStore::load().ok();

    // Include Family-Shared games that aren't installed (and which we don't own),
    // such as titles only available through a family member's library.
    if client.is_authenticated() {
        tracing::info!("Fetching Family Sharing library from Steam ...");
        match client.fetch_family_shared_apps().await {
            Ok(shared) => {
                tracing::info!("Fetched {} Family-Shared apps", shared.len());
                merge_family_shared(&mut games, shared);
            }
            Err(e) => tracing::warn!("could not fetch family shared apps: {e:#}"),
        }
    }

    if installed {
        games.retain(|g| g.is_installed);
    }
    if let Some(needle) = search.as_deref().map(str::to_ascii_lowercase) {
        games.retain(|g| g.name.to_ascii_lowercase().contains(&needle));
    }
    // Filter to a single collection's members when requested. Resolved against
    // the local store (static collections only); an unknown name/id is an error.
    if let Some(name) = collection.as_deref() {
        let store = collections_store
            .as_ref()
            .context("no collections found — run `aurelia collections pull` first")?;
        let col = store.resolve(name)?;
        if col.is_dynamic() {
            bail!(
                "'{}' is a dynamic (filter-based) collection; its membership is computed by \
                 Steam and can't be resolved offline",
                col.name
            );
        }
        let members: std::collections::HashSet<u32> = col.added.iter().copied().collect();
        let removed: std::collections::HashSet<u32> = col.removed.iter().copied().collect();
        games.retain(|g| members.contains(&g.app_id) && !removed.contains(&g.app_id));
    }
    games.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));

    // Precompute each game's static-collection membership names for display.
    let game_collections = |app_id: u32| -> Vec<String> {
        collections_store
            .as_ref()
            .map(|s| {
                s.collections
                    .iter()
                    .filter(|c| !c.deleted && !c.is_dynamic() && c.contains(app_id))
                    .map(|c| c.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    };

    // Resolve the "online required" status only when requested: it needs a PICS
    // appinfo fetch per game, which is too costly to do on every `list`.
    if online {
        if client.is_authenticated() && !client.is_offline() {
            tracing::info!("Resolving online-required status for {} game(s) ...", games.len());
            for g in &mut games {
                match client.fetch_online_required(g.app_id).await {
                    Ok(required) => g.online_required = Some(required),
                    Err(e) => {
                        tracing::warn!("could not determine online-required for {}: {e:#}", g.app_id);
                    }
                }
            }
        } else {
            tracing::warn!(
                "--online needs an authenticated, online session; ONLINE column will be unknown"
            );
        }
    }

    // Resolve `update_available` only when requested: it reads each installed
    // game's appmanifest and fetches its remote depot manifests, too costly for a
    // plain `list`. `check_for_updates` compares local vs remote manifests and is
    // ownership-agnostic, so Family-Shared games (which won't launch when stale)
    // are flagged just like owned ones.
    if check_updates {
        if client.is_authenticated() && !client.is_offline() {
            tracing::info!("Checking {} game(s) for updates ...", games.len());
            if let Err(e) = client.check_for_updates(&mut games).await {
                tracing::warn!("could not check for updates: {e:#}");
            }
        } else {
            tracing::warn!(
                "--check-updates needs an authenticated, online session; update status will be unknown"
            );
        }
    }

    // Record each installed game's depot platform so a driver (e.g. Heroic) can
    // tell native-Linux games from Windows-via-Proton ones. Only installed games
    // have files to inspect; the scan early-exits on the first `.exe`.
    for g in &mut games {
        if g.is_installed {
            if let Some(path) = g.install_path.as_deref() {
                g.platform = detect_installed_platform(path);
            }
        }
    }

    if json {
        // Bake the conventional artwork/store URLs
        let enriched: Vec<serde_json::Value> = games
            .iter()
            .map(|g| {
                let mut v = serde_json::to_value(g).unwrap_or_default();
                if let Some(obj) = v.as_object_mut() {
                    obj.insert(
                        "assets".into(),
                        serde_json::json!({
                            "header": steam_urls::header_url(g.app_id),
                            "capsule": steam_urls::capsule_url(g.app_id),
                            "hero": steam_urls::hero_url(g.app_id),
                            "logo": steam_urls::logo_url(g.app_id),
                        }),
                    );
                    obj.insert("store_url".into(), serde_json::json!(steam_urls::store_url(g.app_id)));
                    obj.insert("collections".into(), serde_json::json!(game_collections(g.app_id)));
                }
                v
            })
            .collect();
        cli_println!("{}", serde_json::to_string_pretty(&enriched)?);
        return Ok(());
    }

    if games.is_empty() {
        cli_println!("No games match.");
        return Ok(());
    }

    if online {
        cli_println!(
            "{:>9}  {:<10}  {:<13}  {:<7}  {:<20}  NAME",
            "APPID", "STATUS", "LICENSE", "ONLINE", "COLLECTIONS"
        );
    } else {
        cli_println!(
            "{:>9}  {:<10}  {:<13}  {:<20}  NAME",
            "APPID", "STATUS", "LICENSE", "COLLECTIONS"
        );
    }
    for g in &games {
        let status = if g.is_installed {
            if g.update_available {
                "update"
            } else {
                "installed"
            }
        } else {
            "-"
        };
        let license = if g.is_owned {
            "owned"
        } else if g.is_family_shared {
            "family-shared"
        } else {
            "unlicensed"
        };
        let branch = if g.active_branch != "public" {
            format!(" [{}]", g.active_branch)
        } else {
            String::new()
        };
        let cols = game_collections(g.app_id);
        let cols = if cols.is_empty() {
            "-".to_string()
        } else {
            cols.join(", ")
        };
        if online {
            let online_col = match g.online_required {
                Some(true) => "yes",
                Some(false) => "no",
                None => "?",
            };
            cli_println!(
                "{:>9}  {:<10}  {:<13}  {:<7}  {:<20}  {}{}",
                g.app_id, status, license, online_col, cols, g.name, branch
            );
        } else {
            cli_println!(
                "{:>9}  {:<10}  {:<13}  {:<20}  {}{}",
                g.app_id, status, license, cols, g.name, branch
            );
        }
    }

    let shared = games.iter().filter(|g| g.is_family_shared).count();
    if shared > 0 {
        cli_println!(
            "\n{} game(s), {} via Family Sharing (not licensed to this account).",
            games.len(),
            shared
        );
    } else {
        cli_println!("\n{} game(s).", games.len());
    }
    Ok(())
}

pub(crate) async fn cmd_account(json: bool) -> Result<()> {
    let client = authed_client().await?;
    let mut data = client.get_account_data().await;
    // Resolve the public persona
    data.persona_name = client.own_persona_name().await;

    if json {
        let value = serde_json::json!({
            "steam_id": data.steam_id,
            "account_name": data.account_name,
            "persona_name": data.persona_name,
            "country": data.country,
            "email": data.email,
            "email_validated": data.email_validated,
            "authed_machines": data.authed_machines,
            "flags": data.flags,
            "vac_bans": data.vac_bans,
            "vac_banned_apps": data.vac_banned_apps,
        });
        cli_println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    cli_println!("Account : {}", data.account_name);
    if let Some(persona) = &data.persona_name {
        cli_println!("Persona : {persona}");
    }
    cli_println!("SteamID : {}", data.steam_id);
    cli_println!("Country : {}", data.country);
    cli_println!(
        "Email   : {} ({})",
        data.email,
        if data.email_validated {
            "validated"
        } else {
            "unvalidated"
        }
    );
    cli_println!("Devices : {}", data.authed_machines);
    cli_println!("VAC bans: {}", data.vac_bans);
    Ok(())
}

/// List the Steam library folders available to install into, with the free
/// space on each, one per line (or a JSON `{ "libraries": [{path, free_bytes}] }`
/// object with `--json`). Only roots that actually contain a `steamapps`
/// directory are reported.
pub(crate) async fn cmd_libraries(json: bool) -> Result<()> {
    let libraries: Vec<(String, Option<u64>)> = aurelia::library::all_library_roots()
        .await
        .into_iter()
        .filter(|root| root.join("steamapps").is_dir())
        .map(|root| {
            let free = available_space_for(&root);
            (root.to_string_lossy().to_string(), free)
        })
        .collect();

    if json {
        let entries: Vec<serde_json::Value> = libraries
            .iter()
            .map(|(path, free)| serde_json::json!({ "path": path, "free_bytes": free }))
            .collect();
        print_json(&serde_json::json!({ "libraries": entries }));
    } else if libraries.is_empty() {
        cli_println!("No Steam library folders found.");
    } else {
        for (path, free) in &libraries {
            match free {
                Some(bytes) => cli_println!("{path}  ({} free)", human_bytes(*bytes)),
                None => cli_println!("{path}"),
            }
        }
    }
    Ok(())
}
