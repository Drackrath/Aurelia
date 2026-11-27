//! `library` command handlers.

use crate::app::output::table::Table;
use crate::app::steam_urls;

use crate::app::commands::common::*;

use anyhow::{Context, Result, bail};

/// Outcome of one bounded install-dir walk.
enum WalkOutcome<T> {
    /// Root missing or unreadable.
    NotDir,
    /// The visitor found its verdict.
    Found(T),
    /// Budget spent before a verdict.
    Exhausted,
    /// Every entry visited.
    Complete,
}

/// Bounded breadth-first walk over regular files.
fn walk_install_dir<T>(
    install_path: &str,
    mut visit: impl FnMut(&std::path::Path) -> Option<T>,
) -> WalkOutcome<T> {
    let root = std::path::Path::new(install_path);
    if !root.is_dir() {
        return WalkOutcome::NotDir;
    }
    let mut queue = std::collections::VecDeque::from([root.to_path_buf()]);
    let mut budget = 100_000usize;
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if budget == 0 {
                return WalkOutcome::Exhausted;
            }
            budget -= 1;
            let path = entry.path();
            if path.is_dir() {
                queue.push_back(path);
                continue;
            }
            if let Some(found) = visit(&path) {
                return WalkOutcome::Found(found);
            }
        }
    }
    WalkOutcome::Complete
}

/// Which platform payloads exist in an install dir: (linux, windows).
/// A dual-depot install (both platforms in one dir) reports both.
/// Partial verdict at budget exhaustion.
pub(crate) fn detect_installed_platform_set(install_path: &str) -> Option<(bool, bool)> {
    let is_elf = |path: &std::path::Path| -> bool {
        use std::io::Read;
        let mut head = [0u8; 4];
        std::fs::File::open(path)
            .ok()
            .and_then(|mut f| f.read_exact(&mut head).ok())
            .is_some_and(|_| head == *b"\x7fELF")
    };
    let mut linux = false;
    let mut windows = false;
    let outcome = walk_install_dir(install_path, |path| {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        match ext.as_deref() {
            Some("exe") => windows = true,
            // ELF check only where a native binary can plausibly live.
            None | Some("so") | Some("x86_64") | Some("appimage") | Some("bin")
                if !linux && is_elf(path) =>
            {
                linux = true;
            }
            _ => {}
        }
        (linux && windows).then_some(())
    });
    match outcome {
        WalkOutcome::NotDir => None,
        _ => Some((linux, windows)),
    }
}

/// Best-effort detection of the platform whose depot is installed for a game, by
/// looking for an app bundle or a Windows executable in its install directory: a
/// macOS build ships an `.app` bundle, a Windows depot always ships a `.exe`, and
/// a native Linux build has neither. Breadth-first so a Windows game's top-level
/// `.exe` is found immediately, and a bundle's `Contents/Info.plist` before any
/// `.exe` a Mono port keeps deeper inside it; the walk is bounded so a large native
/// install can't stall `list`. Returns `None` when the directory can't be read or
/// the budget is exhausted before a verdict (the caller then leaves the platform
/// unknown rather than guessing).
pub(crate) fn detect_installed_platform(install_path: &str) -> Option<String> {
    let has_extension = |path: &std::path::Path, extension: &str| {
        path.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(extension))
    };
    let outcome = walk_install_dir(install_path, |path| {
        let bundle_plist = path.ends_with("Contents/Info.plist")
            && path
                .parent()
                .and_then(std::path::Path::parent)
                .is_some_and(|bundle| has_extension(bundle, "app"));
        if bundle_plist {
            Some("macos".to_string())
        } else {
            has_extension(path, "exe").then(|| "windows".to_string())
        }
    });
    match outcome {
        WalkOutcome::Found(platform) => Some(platform),
        WalkOutcome::Complete => Some("linux".to_string()),
        WalkOutcome::NotDir | WalkOutcome::Exhausted => None,
    }
}

/// Apps whose art needs fetching.
pub(crate) fn art_to_fetch(
    ids: &[u32],
    cache: &std::collections::HashMap<u32, aurelia::core::config::CachedArt>,
    now: u64,
) -> Vec<u32> {
    use aurelia::core::config::ART_CACHE_TTL_SECS;
    ids.iter()
        .copied()
        .filter(|id| {
            cache
                .get(id)
                .is_none_or(|c| c.is_stale(now, ART_CACHE_TTL_SECS))
        })
        .collect()
}

/// Hashed art URLs, else flat.
pub(crate) fn library_art(
    app_id: u32,
    cached: Option<&aurelia::core::config::CachedArt>,
) -> serde_json::Value {
    let store = cached.and_then(|c| c.assets.as_ref());
    let pick = |url: Option<&String>, flat: String| url.cloned().unwrap_or(flat);
    serde_json::json!({
        "header": pick(store.and_then(|a| a.header.as_ref()), steam_urls::header_url(app_id)),
        "capsule": pick(store.and_then(|a| a.capsule.as_ref()), steam_urls::capsule_url(app_id)),
        "hero": pick(store.and_then(|a| a.hero.as_ref()), steam_urls::hero_url(app_id)),
        // StoreBrowse has no logo field.
        "logo": steam_urls::logo_url(app_id),
    })
}

/// Removed/Delisted, store type, else `music/`.
pub(crate) fn library_app_type(
    game: &aurelia::core::models::LibraryGame,
    cached: Option<&aurelia::core::config::CachedArt>,
) -> Option<String> {
    if let Some(label) = cached.and_then(|c| c.availability_label()) {
        return Some(label.to_string());
    }
    cached.and_then(|c| c.app_type.clone()).or_else(|| {
        game.install_path
            .as_deref()
            .map(std::path::Path::new)
            .is_some_and(aurelia::library::is_music_install)
            .then(|| "Soundtrack".to_string())
    })
}

/// Cached art, fetching missing apps once.
async fn resolve_library_art(
    client: &aurelia::steam_client::SteamClient,
    games: &[aurelia::core::models::LibraryGame],
) -> std::collections::HashMap<u32, aurelia::core::config::CachedArt> {
    use aurelia::core::config::{ART_CACHE_VERSION, CachedArt, load_art_cache, save_art_cache};
    let mut cache = load_art_cache().await;
    let now = aurelia::core::time::now_unix();
    let ids: Vec<u32> = games.iter().map(|g| g.app_id).collect();
    let missing = art_to_fetch(&ids, &cache, now);
    if missing.is_empty() || !client.is_authenticated() || client.is_offline() {
        return cache;
    }
    let country = resolve_steam_country(None)
        .await
        .unwrap_or_else(|_| "US".to_string());
    match client.fetch_store_art(&missing, &country).await {
        Ok(mut found) => {
            for id in missing {
                let art = found.remove(&id);
                cache.insert(
                    id,
                    CachedArt {
                        fetched_at: now,
                        assets: art.as_ref().and_then(|a| a.assets.clone()),
                        app_type: art.as_ref().and_then(|a| a.app_type.clone()),
                        status: art.map(|a| a.status),
                        version: ART_CACHE_VERSION,
                    },
                );
            }
            if let Err(e) = save_art_cache(&cache).await {
                tracing::warn!("could not write the art cache: {e:#}");
            }
        }
        Err(e) => tracing::warn!("could not fetch library art: {e:#}"),
    }
    cache
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
    games.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });

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
            tracing::info!(
                "Resolving online-required status for {} game(s) ...",
                games.len()
            );
            for g in &mut games {
                match client.fetch_online_required(g.app_id).await {
                    Ok(required) => g.online_required = Some(required),
                    Err(e) => {
                        tracing::warn!(
                            "could not determine online-required for {}: {e:#}",
                            g.app_id
                        );
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
        if g.is_installed
            && let Some(path) = g.install_path.as_deref()
        {
            g.platform = detect_installed_platform(path);
        }
    }

    // Art cache also carries app types.
    let art = resolve_library_art(&client, &games).await;
    if json {
        let enriched: Vec<serde_json::Value> = games
            .iter()
            .map(|g| {
                let mut v = serde_json::to_value(g).unwrap_or_default();
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("assets".into(), library_art(g.app_id, art.get(&g.app_id)));
                    obj.insert(
                        "type".into(),
                        serde_json::json!(library_app_type(g, art.get(&g.app_id))),
                    );
                    obj.insert(
                        "store_url".into(),
                        serde_json::json!(steam_urls::store_url(g.app_id)),
                    );
                    obj.insert(
                        "collections".into(),
                        serde_json::json!(game_collections(g.app_id)),
                    );
                }
                v
            })
            .collect();
        print_json(&enriched);
        return Ok(());
    }

    if games.is_empty() {
        cli_println!("No games match.");
        return Ok(());
    }

    let mut table = Table::new()
        .right("AppId")
        .left("Status")
        .left("License")
        .left("Type");
    if online {
        table = table.left("Online");
    }
    let mut table = table.left("Collections").left("Name");
    for g in &games {
        let status = if g.is_installed {
            if g.update_available {
                "update"
            } else if g.steam_config_mismatch {
                "mismatch"
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
        let mut branch = branch_suffix(g);
        // Flag games whose only local copy lives in the in-Wine Steam runtime's own
        // library (installed via the in-Wine Steam, e.g. Family-Shared titles) — they
        // launch through the in-Wine Steam rather than Aurelia's Proton pipeline.
        if g.from_windows_steam {
            branch.push_str(" [wine]");
        }
        let kind = library_app_type(g, art.get(&g.app_id)).unwrap_or_else(|| "-".to_string());
        let cols = game_collections(g.app_id);
        let cols = if cols.is_empty() {
            "-".to_string()
        } else {
            cols.join(", ")
        };
        let mut row = vec![
            g.app_id.to_string(),
            status.to_string(),
            license.to_string(),
            kind,
        ];
        if online {
            let online_col = match g.online_required {
                Some(true) => "yes",
                Some(false) => "no",
                None => "?",
            };
            row.push(online_col.to_string());
        }
        row.push(cols);
        row.push(format!("{}{}", g.name, branch));
        table.row(row);
    }
    table.print();

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
    // Only the daemon has one.
    let daemon_logon_id =
        crate::app::daemon::in_daemon().then(aurelia::steam_client::connection::logon_id);

    if json {
        let value = serde_json::json!({
            "daemon_logon_id": daemon_logon_id,
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
        print_json(&value);
        return Ok(());
    }

    if let Some(id) = daemon_logon_id {
        cli_println!("DaemonLogonID: {id}");
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

#[cfg(test)]
mod tests;
