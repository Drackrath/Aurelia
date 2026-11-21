//! Manifest listing, downgrade and pinning.

use super::*;

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
            "pinned_manifests": manifests_json(&pinned_manifests),
        }));
    } else {
        cli_println!(
            "App {app_id}: {}{}",
            if available {
                "available"
            } else {
                "not available"
            },
            if pinned { " (pinned)" } else { "" }
        );
        if let Some(p) = install_path {
            cli_println!("  path: {p}");
        }
    }
    Ok(())
}

/// Stable JSON map, stringified keys.
pub(super) fn manifests_json(
    map: &std::collections::HashMap<u32, u64>,
) -> std::collections::BTreeMap<String, u64> {
    map.iter().map(|(d, m)| (d.to_string(), *m)).collect()
}

/// Turn the parallel `--depot` / `--manifest` lists (or the combined
/// `--manifest <depot>:<manifest>` form) into a depot → manifest override map.
///
/// Bare `--manifest` values pair by position with `--depot`; entries containing a
/// `:` carry their own depot. Rejects unequal parallel-list lengths, duplicate
/// depots, non-numeric ids, and an empty result — all with a clear message.
pub(super) fn parse_manifest_overrides(
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
    let _install_guard = InstallGuard::register(args.app_id, Arc::clone(&state))?;
    let rx = client
        .install_game(
            args.app_id,
            platform,
            InstallOptions {
                cached_vdf: Some(cached_vdf),
                library_override: args.library.clone(),
                manifest_overrides: Some(overrides.clone()),
                branch: args.branch.clone(),
                ..Default::default()
            },
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
            "manifests": manifests_json(&overrides),
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
            "manifests": manifests_json(&manifests),
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
