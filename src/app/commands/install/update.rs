//! Verify, update, language and update checks.

use super::*;

pub(crate) async fn cmd_verify(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    // Registered so `install stop`
    let (_install_guard, state) = InstallGuard::begin(app_id)?;
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
    let (_install_guard, state) = InstallGuard::begin(app_id)?;
    let rx = client
        .update_game(app_id, state)
        .await
        .with_context(|| format!("failed to update app {app_id}"))?;
    drive_progress(rx, json).await?;
    report_operation(app_id, "updated", json);
    Ok(())
}

/// `set-language`: swap a game's language depots.
pub(crate) async fn cmd_set_language(app_id: u32, language: String, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let (_install_guard, state) = InstallGuard::begin(app_id)?;
    let rx = client
        .set_game_language(app_id, &language, state)
        .await
        .with_context(|| format!("failed to switch app {app_id} to {language}"))?;
    drive_progress(rx, json).await?;
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "language": language.trim().to_ascii_lowercase(),
            "status": "language_set",
        }));
    } else {
        cli_println!("App {app_id} now uses {language}.");
    }
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
    updates.sort_by_key(|a| a.name.to_lowercase());

    let mut pinned: Vec<&LibraryGame> = games.iter().filter(|g| is_pinned(g.app_id)).collect();
    pinned.sort_by_key(|a| a.name.to_lowercase());

    let mut mismatched: Vec<&LibraryGame> =
        games.iter().filter(|g| g.steam_config_mismatch).collect();
    mismatched.sort_by_key(|a| a.name.to_lowercase());

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
        let mismatch_arr: Vec<_> = mismatched
            .iter()
            .map(|g| serde_json::json!({ "app_id": g.app_id, "name": g.name }))
            .collect();
        print_json(&serde_json::json!({
            "updates": arr,
            "pinned": pinned_arr,
            "steam_config_mismatch": mismatch_arr,
        }));
        return Ok(());
    }

    if updates.is_empty() {
        cli_println!("All installed games are up to date.");
    } else {
        cli_println!("{:>9}  NAME", "APPID");
        for g in &updates {
            let branch = branch_suffix(g);
            cli_println!("{:>9}  {}{}", g.app_id, g.name, branch);
        }
        cli_println!(
            "\n{} game(s) need an update. Run `aurelia update <app_id>` to install one.",
            updates.len()
        );
    }

    if !mismatched.is_empty() {
        cli_println!(
            "\nSteam wants a different depot set (language or compatibility tool), \
             not a newer build; `aurelia update` can't fix these:"
        );
        for g in &mismatched {
            cli_println!("{:>9}  {}", g.app_id, g.name);
        }
    }

    if !pinned.is_empty() {
        cli_println!("\nPinned (held at a fixed version; `aurelia unpin <app_id>` to release):");
        for g in &pinned {
            cli_println!("{:>9}  {}", g.app_id, g.name);
        }
    }
    Ok(())
}
