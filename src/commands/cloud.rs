//! `cloud` command handlers.

use crate::cli::*;
use crate::commands::common::*;

use std::path::PathBuf;
use anyhow::{Context, Result};

pub(crate) async fn cmd_cloud_sync(
    app_id: u32,
    up: bool,
    down: bool,
    path: Option<PathBuf>,
    resolve: Option<CloudResolve>,
    json: bool,
) -> Result<()> {
    use aurelia::library::cloud_sync::{ConflictPolicy, SyncDirection};

    let client = authed_client().await?;
    let cloud = client.cloud_client()?;

    // Classic (token-less) remote-storage files live under `<appid>/remote`; Auto-Cloud
    // files resolve to real OS paths via their `%RootToken%` prefix. `--path` overrides
    // only the classic base. `%GameInstall%` needs the game's install directory.
    let remote_root = match path {
        Some(p) => p,
        None => aurelia::library::cloud_sync::default_cloud_root(cloud.steam_id(), app_id)
            .context("could not resolve the local cloud save directory")?
            .join("remote"),
    };
    let (_, install_path) = client.is_game_available(app_id).await;
    // A Windows game under Proton writes its saves inside the wine prefix, so the
    // `%Win*%` roots in its cloud filenames only resolve with the prefix in hand.
    let launcher_config = aurelia::core::config::LauncherConfig::load().await?;
    let user_configs = aurelia::core::config::load_user_configs().await?;
    // Must be the prefix the game *runs* in, not the WINEPREFIX the launcher sets:
    // under Proton those differ, and syncing to the wrong one silently leaves the
    // game with no saves.
    let wine_prefix =
        aurelia::core::utils::game_save_prefix(&launcher_config, app_id, &user_configs);
    let resolver = aurelia::library::cloud_sync::CloudPathResolver::new(
        remote_root.clone(),
        install_path.map(PathBuf::from),
    )
    .with_wine_prefix(Some(wine_prefix.clone()));

    // No flag = full sync (down then up); `--down`/`--up` restrict the direction.
    let direction = if up {
        SyncDirection::Up
    } else if down {
        SyncDirection::Down
    } else {
        SyncDirection::Both
    };
    let direction_str = match direction {
        SyncDirection::Up => "up",
        SyncDirection::Down => "down",
        SyncDirection::Both => "both",
    };

    // Without `--resolve` we only *detect* divergent saves (and leave both copies
    // intact); with it, every conflict is resolved by taking the chosen side.
    let policy = resolve.map_or(ConflictPolicy::Detect, ConflictPolicy::from);

    // UFS save rules let the upload pass discover brand-new local saves.
    let specs = client.fetch_ufs_save_specs(app_id).await.unwrap_or_default();
    let outcome = cloud
        .sync(app_id, &resolver, &specs, direction, policy)
        .await
        .with_context(|| format!("cloud sync failed for app {app_id}"))?;

    let status = if outcome.has_failures() {
        "failed"
    } else if outcome.has_conflicts() {
        "conflicts"
    } else if outcome.has_skips() {
        "incomplete"
    } else {
        "ok"
    };

    if json {
        let conflicts: Vec<_> = outcome
            .conflicts
            .iter()
            .map(|c| {
                serde_json::json!({
                    "filename": c.filename,
                    "local_path": c.local_path,
                    "local_hash": c.local_hash,
                    "local_size": c.local_size,
                    "local_timestamp": c.local_timestamp,
                    "cloud_hash": c.cloud_hash,
                    "cloud_size": c.cloud_size,
                    "cloud_timestamp": c.cloud_timestamp,
                })
            })
            .collect();
        print_json(&serde_json::json!({
            "app_id": app_id,
            "direction": direction_str,
            "remote_root": remote_root.to_string_lossy(),
            "status": status,
            "downloaded": outcome.downloaded,
            "uploaded": outcome.uploaded,
            "conflicts": conflicts,
            "skipped": outcome.skipped,
            "skipped_root_tokens": outcome.skipped_tokens(),
            "failed": outcome.failed,
            "wine_prefix": wine_prefix.to_string_lossy(),
        }));
        return Ok(());
    }

    if outcome.has_conflicts() {
        cli_println!(
            "{} Cloud save(s) diverged from local — neither copy was changed:",
            outcome.conflicts.len()
        );
        for c in &outcome.conflicts {
            cli_println!(
                "  {}  (cloud {} bytes @ {}, local {} bytes @ {})",
                c.filename, c.cloud_size, c.cloud_timestamp, c.local_size, c.local_timestamp
            );
        }
        cli_println!(
            "\nResolve with: `aurelia cloud sync {app_id} --resolve cloud`  (use the Steam copy)\n            or `aurelia cloud sync {app_id} --resolve local`  (use the on-disk copy)"
        );
    } else {
        cli_println!(
            "Synced Cloud saves for app {app_id} ({direction_str}): {} down, {} up.",
            outcome.downloaded.len(),
            outcome.uploaded.len()
        );
    }

    // Name the destination. Aurelia can have several prefixes per game and syncing
    // into the wrong one looks exactly like success — the files are written, the
    // counts are right, and the game still starts with no saves.
    cli_println!("Save prefix: {}", wine_prefix.display());

    // A partial restore is the dangerous case: the game finds *some* files and
    // starts as though the save were intact, so this must never read as success.
    if outcome.has_failures() {
        cli_println!(
            "\nERROR: {} Cloud file(s) failed to transfer — this save set is INCOMPLETE:",
            outcome.failed.len()
        );
        for f in outcome.failed.iter().take(SKIP_SAMPLE) {
            cli_println!("  [{}] {}: {}", f.direction, f.filename, f.error);
        }
        if outcome.failed.len() > SKIP_SAMPLE {
            cli_println!("  ... and {} more", outcome.failed.len() - SKIP_SAMPLE);
        }
        cli_println!(
            "\nRe-run `aurelia cloud sync {app_id} --down` to retry just the missing files.\nDo not play until it completes — the game may overwrite a half-restored save."
        );
    }

    // A skipped file was never compared, so the counts above say nothing about it.
    // Report it loudly — the failure mode this replaces was a confident "0 down"
    // against a cloud full of saves.
    if outcome.has_skips() {
        let tokens = outcome.skipped_tokens();
        cli_println!(
            "\nWARNING: {} Cloud file(s) were skipped — Aurelia could not work out where they belong on disk.",
            outcome.skipped.len()
        );
        if tokens.is_empty() {
            for s in outcome.skipped.iter().take(SKIP_SAMPLE) {
                cli_println!("  {}: {}", s.filename, s.reason);
            }
        } else {
            cli_println!(
                "Unmapped save root token(s): {}",
                tokens
                    .iter()
                    .map(|t| format!("%{t}%"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for s in outcome.skipped.iter().take(SKIP_SAMPLE) {
                cli_println!("  {}", s.filename);
            }
        }
        if outcome.skipped.len() > SKIP_SAMPLE {
            cli_println!("  ... and {} more", outcome.skipped.len() - SKIP_SAMPLE);
        }
        if tokens.iter().any(|t| t.starts_with("Win")) {
            cli_println!(
                "\n%Win*% roots live inside the game's Proton prefix. Check that the game is\ninstalled and that its compatdata prefix exists (`aurelia info {app_id}`)."
            );
        } else {
            cli_println!(
                "\nThis token has no mapping in this build — please report it so it can be added."
            );
        }
        cli_println!("These saves are still safe in the Cloud; nothing was overwritten.");
    }
    Ok(())
}

/// How many skipped filenames to print before collapsing into a count — enough to
/// recognise which saves are affected without burying the explanation.
const SKIP_SAMPLE: usize = 5;

pub(crate) async fn cmd_cloud_list(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let cloud = client.cloud_client()?;
    let mut files = cloud
        .get_file_list(app_id)
        .await
        .with_context(|| format!("failed listing Cloud files for app {app_id}"))?;
    files.sort_by(|a, b| a.filename.cmp(&b.filename));

    if json {
        let arr: Vec<_> = files
            .iter()
            .map(|f| {
                serde_json::json!({
                    "filename": f.filename,
                    "size": f.size,
                    "timestamp": f.timestamp,
                    "sha_hash": f.sha_hash,
                })
            })
            .collect();
        print_json(&serde_json::json!({ "app_id": app_id, "files": arr }));
        return Ok(());
    }

    if files.is_empty() {
        cli_println!("No Steam Cloud files for app {app_id}.");
        return Ok(());
    }
    cli_println!("{:>12}  {:<19}  NAME", "SIZE", "MODIFIED");
    let mut total = 0u64;
    for f in &files {
        total += f.size;
        cli_println!(
            "{:>12}  {:<19}  {}",
            human_bytes(f.size),
            format_unix_timestamp(f.timestamp),
            f.filename
        );
    }
    cli_println!("\n{} file(s), {}.", files.len(), human_bytes(total));
    Ok(())
}
