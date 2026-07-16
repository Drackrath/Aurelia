//! `install` command handlers.

use crate::cli::*;
use crate::commands::common::*;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};
use anyhow::{bail, Context, Result};
use aurelia::core::config::load_launcher_config;
use aurelia::core::models::{DepotPlatform, DownloadState, LibraryGame};
use aurelia::steam_client::SteamClient;

pub(crate) async fn cmd_install(
    app_id: u32,
    platform: Option<PlatformArg>,
    restart_steam: bool,
    dry_run: bool,
    library: Option<String>,
    json: bool,
) -> Result<()> {
    let mut client = authed_client().await?;

    // `--dry-run` reports the size estimate and stops — it never touches Steam or
    // downloads anything.
    if dry_run {
        return cmd_install_dry_run(&mut client, app_id, platform, json).await;
    }

    // Note whether this is a DLC so we can refresh Steam's view afterward.
    let is_dlc = client.resolve_dlc_parent(app_id).await.is_some();

    // For a DLC, stop Steam before editing its base appmanifest (Steam overwrites it
    // on exit), then restart it afterward so the running client picks up the change.
    let manage_steam = restart_steam && is_dlc && SteamClient::steam_is_running();
    if manage_steam {
        if !json {
            cli_println!("Stopping Steam ...");
        }
        SteamClient::shutdown_steam()?;
    }

    let (platform, cached_vdf) = match platform {
        Some(p) => (p.into(), None),
        None => {
            let (platforms, buffer) = client
                .get_available_platforms(app_id)
                .await
                .context("failed to detect available platforms")?;
            let chosen = platforms
                .first()
                .copied()
                .unwrap_or(DepotPlatform::Windows);
            if !json {
                cli_println!("Auto-selected platform: {chosen:?}");
            }
            (chosen, Some(buffer))
        }
    };

    // Refuse to start if the target drive can't hold the game. (DLC installs go
    // into the base game's library and aren't covered by the base-game estimate,
    // so they're skipped here.)
    if !is_dlc {
        let est = client
            .estimate_install_size(app_id, platform)
            .await
            .with_context(|| format!("failed to estimate install size for app {app_id}"))?;
        let library_root = match &library {
            Some(lib) => lib.clone(),
            None => load_launcher_config().await?.steam_library_path,
        };
        if let Some(free) = available_space_for(std::path::Path::new(&library_root)) {
            if est.disk_size > free {
                bail!(
                    "not enough space on {library_root}: needs {} but only {} free",
                    human_bytes(est.disk_size),
                    human_bytes(free)
                );
            }
        }
    }

    // Share this install's state via a process-global registry so `install stop`
    // / `install list` (served on other daemon connections) can reach it. The
    // guard removes the entry when this function returns (success, error, abort).
    let state = Arc::new(RwLock::new(DownloadState::default()));
    let _install_guard = InstallGuard::register(app_id, Arc::clone(&state));
    let rx = client
        .install_game(app_id, platform, cached_vdf, None, library, None, None, state)
        .await
        .with_context(|| format!("failed to start install for app {app_id}"))?;
    drive_progress(rx, json).await?;

    let mut steam_restarted = false;
    if manage_steam {
        if !json {
            cli_println!("Starting Steam ...");
        }
        SteamClient::start_steam()?;
        steam_restarted = true;
    }

    // A newly installed DLC is invisible to an already-running Steam client until it
    // re-reads the appmanifest (which it does at startup).
    let steam_restart_required = is_dlc && !manage_steam && SteamClient::steam_is_running();
    if steam_restart_required && !json {
        cli_eprintln!();
        cli_eprintln!("Note: the DLC content is installed, but a running Steam client reads DLC state");
        cli_eprintln!("      only at startup. Restart Steam (or re-run with --restart-steam) for it to");
        cli_eprintln!("      be recognized in-game.");
    }

    if json {
        print_json_line(&serde_json::json!({
            "event": "result",
            "app_id": app_id,
            "status": "installed",
            "dlc": is_dlc,
            "steam_restart_required": steam_restart_required,
            "steam_restarted": steam_restarted,
        }));
    }
    Ok(())
}

/// Report the estimated download/disk size for installing `app_id` without
/// installing anything (mirrors Nile's `install --info --json`).
pub(crate) async fn cmd_install_dry_run(
    client: &mut SteamClient,
    app_id: u32,
    platform: Option<PlatformArg>,
    json: bool,
) -> Result<()> {
    let platform: DepotPlatform = match platform {
        Some(p) => p.into(),
        None => {
            let (platforms, _) = client
                .get_available_platforms(app_id)
                .await
                .context("failed to detect available platforms")?;
            platforms.first().copied().unwrap_or(DepotPlatform::Windows)
        }
    };

    let est = client
        .estimate_install_size(app_id, platform)
        .await
        .with_context(|| format!("failed to estimate install size for app {app_id}"))?;

    let platform_str = format!("{platform:?}").to_lowercase();
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "platform": platform_str,
            "download_size": est.download_size,
            "disk_size": est.disk_size,
            "depot_count": est.depot_count,
        }));
    } else {
        cli_println!("Install estimate for app {app_id} ({platform_str}):");
        cli_println!("  Download size: {}", human_bytes(est.download_size));
        cli_println!("  Disk size    : {}", human_bytes(est.disk_size));
        cli_println!("  Depots       : {}", est.depot_count);
    }
    Ok(())
}

/// `aurelia install stop <app_id>`: signal a running install (tracked in this
/// process's registry) to abort. The download loops poll `abort_signal`.
pub(crate) async fn cmd_install_stop(app_id: u32, json: bool) -> Result<()> {
    let state = active_installs()
        .lock()
        .ok()
        .and_then(|map| map.get(&app_id).cloned());

    let Some(state) = state else {
        if json {
            print_json(&serde_json::json!({
                "event": "not_found",
                "app_id": app_id,
            }));
        } else {
            cli_eprintln!(
                "no active install for app {app_id} (is the daemon running, and is an install in progress?)"
            );
        }
        return Ok(());
    };

    // The abort flag is an `Arc<AtomicBool>` inside the struct; we only need to
    // read the struct to flip it (no write lock required).
    if let Ok(guard) = state.read() {
        guard.abort_signal.store(true, Ordering::SeqCst);
    }

    if json {
        print_json(&serde_json::json!({
            "event": "stopping",
            "app_id": app_id,
        }));
    } else {
        cli_println!("Stopping install of app {app_id} ...");
    }
    Ok(())
}

/// `aurelia install list`: report the installs in flight in this process.
pub(crate) async fn cmd_install_list(json: bool) -> Result<()> {
    // Snapshot under the lock, then release it before printing.
    let mut rows: Vec<(u32, String, u64, u64, String, bool)> = Vec::new();
    if let Ok(map) = active_installs().lock() {
        for state in map.values() {
            if let Ok(s) = state.read() {
                rows.push((
                    s.app_id,
                    s.app_name.clone(),
                    s.total_bytes,
                    s.downloaded_bytes,
                    s.status_text.clone(),
                    s.is_downloading,
                ));
            }
        }
    }
    rows.sort_by_key(|r| r.0);

    if json {
        let arr: Vec<_> = rows
            .iter()
            .map(|(app_id, name, total, done, status, downloading)| {
                serde_json::json!({
                    "app_id": app_id,
                    "name": name,
                    "downloaded_bytes": done,
                    "total_bytes": total,
                    "percent": percent_of(*done, *total),
                    "status": status,
                    "is_downloading": downloading,
                })
            })
            .collect();
        print_json(&serde_json::Value::Array(arr));
        return Ok(());
    }

    if rows.is_empty() {
        cli_println!("No installs in progress.");
        return Ok(());
    }

    cli_println!("{:>9}  {:>20}  {:>9}  NAME / STATUS", "APPID", "PROGRESS", "");
    for (app_id, name, total, done, status, _) in &rows {
        let progress = format!("{} / {}", human_bytes(*done), human_bytes(*total));
        cli_println!(
            "{:>9}  {:>20}  {:>5.1}%  {}",
            app_id,
            progress,
            percent_of(*done, *total),
            if name.is_empty() { status } else { name }
        );
    }
    Ok(())
}

pub(crate) async fn cmd_uninstall(app_id: u32, delete_prefix: bool, json: bool) -> Result<()> {
    let client = authed_client().await?;
    client
        .uninstall_game(app_id, delete_prefix)
        .await
        .with_context(|| format!("failed to uninstall app {app_id}"))?;
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "uninstalled": true,
            "deleted_prefix": delete_prefix,
        }));
    } else {
        cli_println!("Uninstalled app {app_id}.");
    }
    Ok(())
}

pub(crate) async fn cmd_move(
    app_id: u32,
    library: PathBuf,
    restart_steam: bool,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;

    // Steam rewrites appmanifests and libraryfolders.vdf on exit, so it must not be
    // running while we move things.
    let managed = steam_guard_stop(restart_steam, json)?;
    let rx = client
        .move_install(app_id, library.clone())
        .await
        .with_context(|| format!("failed to start moving app {app_id}"))?;
    let outcome = drive_progress(rx, json).await;
    steam_guard_restart(managed, json)?;
    outcome.with_context(|| format!("failed to move app {app_id}"))?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "status": "moved",
            "library": library.to_string_lossy(),
            "steam_restarted": managed,
        }));
    } else {
        cli_println!("Moved app {app_id} to {}.", library.display());
    }
    Ok(())
}

pub(crate) async fn cmd_relink(
    app_id: u32,
    library: PathBuf,
    restart_steam: bool,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;
    let managed = steam_guard_stop(restart_steam, json)?;
    let result = client.relink_install(app_id, library.clone()).await;
    steam_guard_restart(managed, json)?;
    let path = result.with_context(|| format!("failed to relink app {app_id}"))?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "status": "relinked",
            "library": library.to_string_lossy(),
            "install_path": path.to_string_lossy(),
            "steam_restarted": managed,
        }));
    } else {
        cli_println!("Relinked app {app_id} to {}.", library.display());
    }
    Ok(())
}

pub(crate) async fn cmd_import(
    app_id: u32,
    library: PathBuf,
    platform: Option<PlatformArg>,
    restart_steam: bool,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;
    // Default to the OS we're running on for the depot/platform match.
    let platform: DepotPlatform = platform.map(Into::into).unwrap_or(if cfg!(target_os = "windows") {
        DepotPlatform::Windows
    } else {
        DepotPlatform::Linux
    });

    let managed = steam_guard_stop(restart_steam, json)?;
    let result = client.import_install(app_id, library.clone(), platform).await;
    steam_guard_restart(managed, json)?;
    let path = result.with_context(|| format!("failed to import app {app_id}"))?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "status": "imported",
            "library": library.to_string_lossy(),
            "install_path": path.to_string_lossy(),
            "steam_restarted": managed,
        }));
    } else {
        cli_println!("Imported app {app_id} from {}.", path.display());
    }
    Ok(())
}

pub(crate) async fn cmd_available(app_id: u32, json: bool) -> Result<()> {
    // `is_game_available` only reads the local appmanifest and checks the files on
    // disk, so we deliberately build a client *without* restoring the Steam session.
    // A driver like Heroic calls `available` per game on every refresh; restoring
    // the session here would mean one Steam CM logon per call (and Steam throttles
    // repeated logons hard) for data we never fetch over the wire.
    let client = SteamClient::new()?;
    let (available, install_path) = client.is_game_available(app_id).await;
    let (pinned, pinned_manifests) = aurelia::core::config::game_pin_state(app_id).await;
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "available": available,
            "install_path": install_path,
            "pinned": pinned,
            "pinned_manifests": pinned_manifests
                .iter()
                .map(|(d, m)| (d.to_string(), *m))
                .collect::<std::collections::BTreeMap<String, u64>>(),
        }));
    } else {
        cli_println!(
            "App {app_id}: {}{}",
            if available { "available" } else { "not available" },
            if pinned { " (pinned)" } else { "" }
        );
        if let Some(p) = install_path {
            cli_println!("  path: {p}");
        }
    }
    Ok(())
}

pub(crate) async fn cmd_verify(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let state = Arc::new(RwLock::new(DownloadState::default()));
    let rx = client
        .verify_game(app_id, state)
        .await
        .with_context(|| format!("failed to verify app {app_id}"))?;
    drive_progress(rx, json).await?;
    report_operation(app_id, "verified", json);
    Ok(())
}

pub(crate) async fn cmd_update(app_id: u32, force: bool, json: bool) -> Result<()> {
    // A pinned (downgraded) game must not be silently upgraded. `--force` overrides.
    let (pinned, _) = aurelia::core::config::game_pin_state(app_id).await;
    if pinned && !force {
        bail!("app {app_id} is pinned — run `aurelia unpin {app_id}` first (or pass --force)");
    }

    let client = authed_client().await?;
    let state = Arc::new(RwLock::new(DownloadState::default()));
    let rx = client
        .update_game(app_id, state)
        .await
        .with_context(|| format!("failed to update app {app_id}"))?;
    drive_progress(rx, json).await?;
    report_operation(app_id, "updated", json);
    Ok(())
}

/// `aurelia update` with no app id: list every installed game whose active branch
/// has a newer build available than what's on disk.
pub(crate) async fn cmd_check_updates(json: bool) -> Result<()> {
    let mut client = authed_client().await?;
    if client.is_offline() {
        bail!("offline — connect to Steam to check for updates");
    }

    let mut games = load_library(&mut client).await;
    games.retain(|g| g.is_installed);
    if games.is_empty() {
        if json {
            print_json(&serde_json::json!({ "updates": [] }));
        } else {
            cli_println!("No installed games found.");
        }
        return Ok(());
    }

    tracing::info!("Checking {} installed game(s) for updates ...", games.len());
    client.check_for_updates(&mut games).await?;

    // A pinned game is deliberately held at an older build — report it as pinned,
    // never as "update available".
    let cfg = load_launcher_config().await?;
    let is_pinned = |app_id: u32| cfg.game_configs.get(&app_id).is_some_and(|g| g.pinned);

    let mut updates: Vec<&LibraryGame> = games
        .iter()
        .filter(|g| g.update_available && !is_pinned(g.app_id))
        .collect();
    updates.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let mut pinned: Vec<&LibraryGame> = games.iter().filter(|g| is_pinned(g.app_id)).collect();
    pinned.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    if json {
        let arr: Vec<_> = updates
            .iter()
            .map(|g| {
                serde_json::json!({
                    "app_id": g.app_id,
                    "name": g.name,
                    "active_branch": g.active_branch,
                })
            })
            .collect();
        let pinned_arr: Vec<_> = pinned
            .iter()
            .map(|g| serde_json::json!({ "app_id": g.app_id, "name": g.name }))
            .collect();
        print_json(&serde_json::json!({ "updates": arr, "pinned": pinned_arr }));
        return Ok(());
    }

    if updates.is_empty() {
        cli_println!("All installed games are up to date.");
    } else {
        cli_println!("{:>9}  NAME", "APPID");
        for g in &updates {
            let branch = if g.active_branch != "public" {
                format!(" [{}]", g.active_branch)
            } else {
                String::new()
            };
            cli_println!("{:>9}  {}{}", g.app_id, g.name, branch);
        }
        cli_println!(
            "\n{} game(s) need an update. Run `aurelia update <app_id>` to install one.",
            updates.len()
        );
    }

    if !pinned.is_empty() {
        cli_println!("\nPinned (held at a fixed version; `aurelia unpin <app_id>` to release):");
        for g in &pinned {
            cli_println!("{:>9}  {}", g.app_id, g.name);
        }
    }
    Ok(())
}

/// Turn the parallel `--depot` / `--manifest` lists (or the combined
/// `--manifest <depot>:<manifest>` form) into a depot → manifest override map.
///
/// Bare `--manifest` values pair by position with `--depot`; entries containing a
/// `:` carry their own depot. Rejects unequal parallel-list lengths, duplicate
/// depots, non-numeric ids, and an empty result — all with a clear message.
fn parse_manifest_overrides(
    depots: &[u32],
    manifests: &[String],
) -> Result<std::collections::HashMap<u32, u64>> {
    use std::collections::HashMap;
    let mut map: HashMap<u32, u64> = HashMap::new();
    let mut bare: Vec<u64> = Vec::new();

    for m in manifests {
        if let Some((d, mm)) = m.split_once(':') {
            let depot: u32 = d
                .trim()
                .parse()
                .with_context(|| format!("invalid depot id in --manifest {m:?}"))?;
            let manifest: u64 = mm
                .trim()
                .parse()
                .with_context(|| format!("invalid manifest id in --manifest {m:?}"))?;
            if map.insert(depot, manifest).is_some() {
                bail!("depot {depot} specified more than once");
            }
        } else {
            let manifest: u64 = m.trim().parse().with_context(|| {
                format!("invalid manifest id {m:?} (use a number, or <depot>:<manifest>)")
            })?;
            bare.push(manifest);
        }
    }

    if !bare.is_empty() || !depots.is_empty() {
        if depots.len() != bare.len() {
            bail!(
                "--depot and --manifest must be given in equal numbers (got {} depot(s) and {} bare manifest(s)); \
                 pair each --depot with a --manifest, or use --manifest <depot>:<manifest>",
                depots.len(),
                bare.len()
            );
        }
        for (d, m) in depots.iter().zip(bare) {
            if map.insert(*d, m).is_some() {
                bail!("depot {d} specified more than once");
            }
        }
    }

    if map.is_empty() {
        bail!("provide at least one depot/manifest pair, e.g. --depot 1234 --manifest 5678");
    }
    Ok(map)
}

/// `aurelia manifests <app_id>`: list each depot's current manifest id per branch
/// (version discovery). Steam only exposes *current* ids; the printed SteamDB
/// links are where historical/older ids can be found for a `downgrade`.
pub(crate) async fn cmd_manifests(app_id: u32, depot: Option<u32>, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let mut manifests = client
        .list_depot_manifests(app_id)
        .await
        .with_context(|| format!("failed listing manifests for app {app_id}"))?;
    if let Some(d) = depot {
        manifests.retain(|m| m.depot_id == d);
    }

    if json {
        let arr: Vec<_> = manifests
            .iter()
            .map(|m| {
                serde_json::json!({
                    "depot_id": m.depot_id,
                    "depot_name": m.depot_name,
                    "branch": m.branch,
                    "manifest_id": m.manifest_id,
                    "size": m.size,
                })
            })
            .collect();
        print_json(&serde_json::Value::Array(arr));
        return Ok(());
    }

    if manifests.is_empty() {
        cli_println!(
            "No depot manifests found for app {app_id}{}.",
            depot.map(|d| format!(" (depot {d})")).unwrap_or_default()
        );
        return Ok(());
    }

    cli_println!(
        "{:>9}  {:<14}  {:>20}  {:>12}  NAME",
        "DEPOT",
        "BRANCH",
        "MANIFEST_ID",
        "SIZE"
    );
    for m in &manifests {
        cli_println!(
            "{:>9}  {:<14}  {:>20}  {:>12}  {}",
            m.depot_id,
            m.branch,
            m.manifest_id,
            human_bytes(m.size),
            m.depot_name.as_deref().unwrap_or("")
        );
    }

    // Historical ids aren't in Steam's data — point at each depot's SteamDB page.
    let mut depot_ids: Vec<u32> = manifests.iter().map(|m| m.depot_id).collect();
    depot_ids.sort_unstable();
    depot_ids.dedup();
    cli_println!("\nOnly current manifest ids are shown (Steam does not expose older ones).");
    cli_println!("Find historical manifest ids for a downgrade on SteamDB:");
    for d in depot_ids {
        cli_println!("  https://steamdb.info/depot/{d}/manifests/");
    }
    Ok(())
}

/// `aurelia downgrade`: install specific (usually older) depot manifests and pin
/// them, streaming progress via the same path as `install`.
pub(crate) async fn cmd_downgrade(args: DowngradeArgs, json: bool) -> Result<()> {
    let overrides = parse_manifest_overrides(&args.depots, &args.manifests)?;

    if args.branch_password.is_some() && !json {
        cli_eprintln!(
            "Note: --branch-password is recorded for reference only; the manifest ids you supply are downloaded directly."
        );
    }

    let mut client = authed_client().await?;

    // Auto-detect the platform for the non-pinned depots; overridden depots are
    // force-included by install_game regardless of platform.
    let (platforms, cached_vdf) = client
        .get_available_platforms(args.app_id)
        .await
        .context("failed to detect available platforms")?;
    let platform = platforms.first().copied().unwrap_or(DepotPlatform::Windows);

    if !json {
        cli_println!(
            "Downgrading app {} — pinning {} depot(s):",
            args.app_id,
            overrides.len()
        );
        let mut pairs: Vec<(&u32, &u64)> = overrides.iter().collect();
        pairs.sort();
        for (d, m) in pairs {
            cli_println!("  depot {d} -> manifest {m}");
        }
    }

    let state = Arc::new(RwLock::new(DownloadState::default()));
    let _install_guard = InstallGuard::register(args.app_id, Arc::clone(&state));
    let rx = client
        .install_game(
            args.app_id,
            platform,
            Some(cached_vdf),
            None,
            args.library.clone(),
            Some(overrides.clone()),
            args.branch.clone(),
            Arc::clone(&state),
        )
        .await
        .with_context(|| format!("failed to start downgrade for app {}", args.app_id))?;
    drive_progress(rx, json).await?;

    // Optional integrity pass. `verify_game` checks against the just-written
    // (downgraded) manifest ids in the appmanifest, not the current build.
    if args.verify {
        if !json {
            cli_println!("Verifying ...");
        }
        let vstate = Arc::new(RwLock::new(DownloadState::default()));
        let vrx = client
            .verify_game(args.app_id, vstate)
            .await
            .with_context(|| format!("failed to verify app {}", args.app_id))?;
        drive_progress(vrx, json).await?;
    }

    let pinned = !args.no_pin;
    if pinned {
        aurelia::core::config::set_game_pin(args.app_id, overrides.clone())
            .await
            .with_context(|| {
                format!(
                    "downgrade succeeded but failed to record the pin for app {}",
                    args.app_id
                )
            })?;
    }

    if json {
        print_json(&serde_json::json!({
            "app_id": args.app_id,
            "status": "downgraded",
            "pinned": pinned,
            "manifests": overrides
                .iter()
                .map(|(d, m)| (d.to_string(), *m))
                .collect::<std::collections::BTreeMap<String, u64>>(),
        }));
    } else {
        cli_println!(
            "Downgraded app {}{}.",
            args.app_id,
            if pinned { " and pinned it" } else { "" }
        );
        if pinned {
            cli_println!(
                "Aurelia's `update` / `check-updates` will now hold it here. Note: launching"
            );
            cli_println!(
                "through the OFFICIAL Steam client can still re-queue an update (the pin is"
            );
            cli_println!("authoritative only for Aurelia's own commands).");
        }
    }
    Ok(())
}

/// `aurelia pin <app_id>`: lock Aurelia's update commands for a game, recording
/// its currently-installed manifests.
pub(crate) async fn cmd_pin(app_id: u32, json: bool) -> Result<()> {
    // Reading the local appmanifest needs no Steam session.
    let client = SteamClient::new()?;
    let manifests = client
        .installed_depot_manifests(app_id)
        .await
        .with_context(|| format!("failed reading installed manifests for app {app_id}"))?;
    if manifests.is_empty() {
        bail!("app {app_id} has no installed depots to pin (is it installed?)");
    }
    aurelia::core::config::set_game_pin(app_id, manifests.clone()).await?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "pinned": true,
            "manifests": manifests
                .iter()
                .map(|(d, m)| (d.to_string(), *m))
                .collect::<std::collections::BTreeMap<String, u64>>(),
        }));
    } else {
        cli_println!(
            "Pinned app {app_id} to its installed manifests ({} depot(s)). Aurelia won't update it until `aurelia unpin {app_id}`.",
            manifests.len()
        );
    }
    Ok(())
}

/// `aurelia unpin <app_id>`: release a game's version pin.
pub(crate) async fn cmd_unpin(app_id: u32, json: bool) -> Result<()> {
    let (was_pinned, _) = aurelia::core::config::game_pin_state(app_id).await;
    aurelia::core::config::clear_game_pin(app_id).await?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "pinned": false,
            "was_pinned": was_pinned,
        }));
    } else if was_pinned {
        cli_println!("Unpinned app {app_id}.");
    } else {
        cli_println!("App {app_id} was not pinned.");
    }
    Ok(())
}

#[cfg(test)]
mod downgrade_tests {
    use super::parse_manifest_overrides;

    #[test]
    fn parallel_lists_pair_by_position() {
        let map = parse_manifest_overrides(&[10, 20], &["111".into(), "222".into()]).unwrap();
        assert_eq!(map.get(&10), Some(&111));
        assert_eq!(map.get(&20), Some(&222));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn combined_form_carries_its_own_depot() {
        let map = parse_manifest_overrides(&[], &["10:111".into(), "20:222".into()]).unwrap();
        assert_eq!(map.get(&10), Some(&111));
        assert_eq!(map.get(&20), Some(&222));
    }

    #[test]
    fn mixing_bare_and_combined_is_supported() {
        // One combined entry plus one parallel pair.
        let map = parse_manifest_overrides(&[20], &["10:111".into(), "222".into()]).unwrap();
        assert_eq!(map.get(&10), Some(&111));
        assert_eq!(map.get(&20), Some(&222));
    }

    #[test]
    fn unequal_parallel_lists_are_rejected() {
        let err = parse_manifest_overrides(&[10, 20], &["111".into()]).unwrap_err();
        assert!(err.to_string().contains("equal numbers"), "got: {err}");
    }

    #[test]
    fn empty_input_is_rejected() {
        let err = parse_manifest_overrides(&[], &[]).unwrap_err();
        assert!(err.to_string().contains("at least one"), "got: {err}");
    }

    #[test]
    fn duplicate_depot_is_rejected() {
        let err =
            parse_manifest_overrides(&[10], &["10:111".into(), "222".into()]).unwrap_err();
        assert!(err.to_string().contains("more than once"), "got: {err}");
    }

    #[test]
    fn non_numeric_ids_are_rejected() {
        assert!(parse_manifest_overrides(&[], &["abc".into()]).is_err());
        assert!(parse_manifest_overrides(&[], &["10:xyz".into()]).is_err());
    }
}
