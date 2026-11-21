//! Install, dry run, resume and backlog.

use super::*;

/// Prefer the Windows depot; Proton runs it on Linux.
///
/// Native Linux builds are often older or fussier than the Windows build under
/// Proton/DXVK, so Windows is the default everywhere; `--platform linux` opts
/// into the native build explicitly.
pub(super) fn auto_select_platform(platforms: &[DepotPlatform]) -> DepotPlatform {
    platforms
        .iter()
        .copied()
        .find(|p| *p == DepotPlatform::Windows)
        .or_else(|| platforms.first().copied())
        .unwrap_or(DepotPlatform::Windows)
}

pub(crate) async fn cmd_install(
    app_id: u32,
    platform: Option<PlatformArg>,
    restart_steam: bool,
    dry_run: bool,
    library: Option<String>,
    json: bool,
) -> Result<()> {
    // `--dry-run` reports the size estimate and stops — it never touches Steam or
    // downloads anything.
    if dry_run {
        let mut client = authed_client().await?;
        return cmd_install_dry_run(&mut client, app_id, platform, json).await;
    }
    let request = InstallRequest {
        app_id,
        platform,
        restart_steam,
        library,
        on_disk: 0,
        auto_attempts: 0,
    };
    run_install(request, json).await
}

/// One install, fresh or resumed.
pub(crate) struct InstallRequest {
    pub(crate) app_id: u32,
    pub(crate) platform: Option<PlatformArg>,
    pub(crate) restart_steam: bool,
    pub(crate) library: Option<String>,
    /// Bytes a resume already has.
    pub(crate) on_disk: u64,
    pub(crate) auto_attempts: u32,
}

/// Install, keeping the backlog in step.
pub(crate) async fn run_install(request: InstallRequest, json: bool) -> Result<()> {
    let InstallRequest {
        app_id,
        platform,
        restart_steam,
        library,
        on_disk,
        auto_attempts,
    } = request;
    let mut client = authed_client().await?;

    // Note whether this is a DLC so we can refresh Steam's view afterward.
    let is_dlc = client.resolve_dlc_parent(app_id).await.is_some();

    let (platform, cached_vdf) = match platform {
        Some(p) => (p.into(), None),
        None => {
            let (platforms, buffer) = client
                .get_available_platforms(app_id)
                .await
                .context("failed to detect available platforms")?;
            let chosen = auto_select_platform(&platforms);
            if !json {
                cli_println!("Auto-selected platform: {chosen:?}");
            }
            (chosen, Some(buffer))
        }
    };

    // Refuse to start if the target drive can't hold the game. (DLC installs go
    // into the base game's library and aren't covered by the base-game estimate,
    // so they're skipped here.)
    let library_root = match &library {
        Some(lib) => lib.clone(),
        None => load_launcher_config().await?.steam_library_path,
    };
    if !is_dlc {
        let est = client
            .estimate_install_size(app_id, platform)
            .await
            .with_context(|| format!("failed to estimate install size for app {app_id}"))?;
        if let Some(free) = available_space_for(std::path::Path::new(&library_root)) {
            // A resume's partial files already count.
            if est.disk_size.saturating_sub(on_disk) > free {
                return Err(aurelia::core::error::TypedError::new(
                    aurelia::core::error::ErrorKind::DiskFull,
                    format!(
                        "not enough space on {library_root}: needs {} but only {} free",
                        human_bytes(est.disk_size),
                        human_bytes(free)
                    ),
                )
                .into());
            }
        }
    }

    // Share this install's state via a process-global registry so `install stop`
    // / `install list` (served on other daemon connections) can reach it. The
    // guard removes the entry when this function returns (success, error, abort).
    // Verified chunks count themselves; don't seed.
    let state = Arc::new(RwLock::new(DownloadState::default()));
    let _install_guard = InstallGuard::register(app_id, Arc::clone(&state))?;

    // Stop late; nothing below `?`-returns.
    let manage_steam = steam_guard_stop_optional(restart_steam && is_dlc, json)?;

    // Recorded first, so a crash stays resumable.
    let platform_name = format!("{platform:?}").to_lowercase();
    let started = aurelia::core::time::now_unix();
    let mut record = backlog::BacklogEntry {
        app_id,
        name: String::new(),
        state: backlog::BacklogState::Interrupted,
        platform: platform_name,
        library: library_root.clone(),
        install_dir: None,
        is_dlc,
        restart_steam,
        downloaded_bytes: on_disk,
        total_bytes: 0,
        reason: Some("install did not finish".into()),
        error_type: None,
        auto_attempts,
        added_at: started,
        updated_at: started,
    };
    note_backlog(backlog::upsert(record.clone()));

    let outcome = match client
        .install_game(
            app_id,
            platform,
            InstallOptions {
                cached_vdf,
                library_override: Some(library_root.clone()),
                ..Default::default()
            },
            Arc::clone(&state),
        )
        .await
    {
        Ok(rx) => {
            // `install_game` resolves and creates the target before returning.
            // Persist it now so a daemon crash still leaves `forget --delete-files`
            // with enough information to safely clean up the partial directory.
            if let Ok(s) = state.read() {
                record.name = s.app_name.clone();
                record.install_dir = s.install_dir.clone();
                record.total_bytes = s.total_bytes;
            }
            note_backlog(backlog::upsert(record.clone()));
            match drive_progress_until(rx, json).await {
                Ok(true) => Ok(()),
                Ok(false) if stop_requested(&state) => Err(anyhow::anyhow!(TypedError::new(
                    ErrorKind::Cancelled,
                    "download stopped"
                ))),
                Ok(false) => bail!("install of app {app_id} ended without a result"),
                Err(err) => Err(err),
            }
        }
        Err(err) => Err(err).with_context(|| format!("failed to start install for app {app_id}")),
    };
    // Always restore a Steam client that we stopped, including when the download
    // failed or was cancelled. Otherwise a failed DLC install can leave Steam
    // stopped and hide the original install error behind cleanup failure.
    let settled = finish_backlog(app_id, record, &state, outcome);
    settle_with_restart(settled, || steam_guard_restart(manage_steam, json))?;
    let steam_restarted = manage_steam;

    // A newly installed DLC is invisible to an already-running Steam client until it
    // re-reads the appmanifest (which it does at startup).
    let steam_restart_required = is_dlc && !manage_steam && SteamClient::steam_is_running();
    if steam_restart_required && !json {
        cli_eprintln!();
        cli_eprintln!(
            "Note: the DLC content is installed, but a running Steam client reads DLC state"
        );
        cli_eprintln!(
            "      only at startup. Restart Steam (or re-run with --restart-steam) for it to"
        );
        cli_eprintln!("      be recognized in-game.");
    }

    if json {
        print_result_line(serde_json::json!({
            "app_id": app_id,
            "status": "installed",
            "dlc": is_dlc,
            "steam_restart_required": steam_restart_required,
            "steam_restarted": steam_restarted,
        }));
    }
    Ok(())
}

/// Log, never fail on, backlog writes.
pub(super) fn note_backlog<T>(result: Result<T>) {
    if let Err(e) = result {
        tracing::warn!("install backlog not updated: {e:#}");
    }
}

/// Whether pause or stop was asked.
pub(super) fn stop_requested(state: &Arc<RwLock<DownloadState>>) -> bool {
    state
        .read()
        .map(|s| s.abort_signal.load(Ordering::SeqCst))
        .unwrap_or(false)
}

/// Settle the backlog entry from `outcome`.
pub(super) fn finish_backlog(
    app_id: u32,
    mut record: backlog::BacklogEntry,
    state: &Arc<RwLock<DownloadState>>,
    outcome: Result<()>,
) -> Result<()> {
    let Err(err) = outcome else {
        note_backlog(backlog::remove(app_id));
        return Ok(());
    };
    let paused = state.read().map(|s| s.is_paused).unwrap_or(false);
    if let Ok(s) = state.read() {
        record.name = s.app_name.clone();
        record.install_dir = s.install_dir.clone();
        record.downloaded_bytes = record.downloaded_bytes.max(s.downloaded_bytes);
        record.total_bytes = s.total_bytes;
    }
    let kind = classify(&err).kind;
    record.updated_at = aurelia::core::time::now_unix();
    let err = if kind == ErrorKind::Cancelled {
        record.reason = None;
        record.error_type = None;
        if paused {
            record.state = backlog::BacklogState::Paused;
            anyhow::anyhow!(TypedError::new(
                ErrorKind::Cancelled,
                format!(
                    "install of app {app_id} paused; continue with `aurelia install resume {app_id}`"
                ),
            ))
        } else {
            record.state = backlog::BacklogState::Aborted;
            err.context(format!(
                "install of app {app_id} stopped; it stays in the backlog"
            ))
        }
    } else {
        record.state = backlog::BacklogState::Interrupted;
        record.reason = Some(format!("{err:#}"));
        record.error_type = serde_json::to_value(kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string));
        err
    };
    note_backlog(backlog::upsert(record));
    Err(err)
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
            auto_select_platform(&platforms)
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

/// `aurelia install resume <app_id>|--all`.
pub(crate) async fn cmd_install_resume(app_id: Option<u32>, all: bool, json: bool) -> Result<()> {
    let entries = backlog::load()?;
    let ids: Vec<u32> = if all {
        entries.iter().map(|e| e.app_id).collect()
    } else {
        let id = app_id.unwrap_or_default();
        if !entries.iter().any(|e| e.app_id == id) {
            return Err(TypedError::new(
                ErrorKind::NotFound,
                format!("app {id} is not in the install backlog (see `aurelia install list`)"),
            )
            .into());
        }
        vec![id]
    };
    if ids.is_empty() {
        if !json {
            cli_println!("The install backlog is empty.");
        }
        return Ok(());
    }
    resume_entries(&ids, false, json).await
}

/// Resume `ids` one after another.
///
/// `auto` counts attempts; manual resets them.
pub(crate) async fn resume_entries(ids: &[u32], auto: bool, json: bool) -> Result<()> {
    let mut failures: Vec<(u32, anyhow::Error)> = Vec::new();
    for &app_id in ids {
        let Some(entry) = backlog::get(app_id)? else {
            continue;
        };
        if active_installs()
            .lock()
            .is_ok_and(|m| m.contains_key(&app_id))
        {
            continue;
        }
        if !json {
            let name = if entry.name.is_empty() {
                format!("app {app_id}")
            } else {
                entry.name.clone()
            };
            cli_println!("Resuming {name} ({}) ...", entry.state.as_str());
        }
        // Counted now, so early failures count.
        let auto_attempts = if auto {
            match backlog::bump_auto_attempts(app_id) {
                Ok(Some(n)) => n,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!("install backlog not updated: {e:#}");
                    continue;
                }
            }
        } else {
            0
        };
        let request = InstallRequest {
            app_id,
            platform: Some(if entry.platform == "linux" {
                PlatformArg::Linux
            } else {
                PlatformArg::Windows
            }),
            restart_steam: entry.restart_steam,
            library: Some(entry.library.clone()),
            on_disk: entry.downloaded_bytes,
            auto_attempts,
        };
        if let Err(e) = run_install(request, json).await {
            if ids.len() == 1 {
                return Err(e);
            }
            // Report it and keep going.
            let kind = classify(&e).kind;
            if json {
                print_result_line(serde_json::json!({
                    "app_id": app_id,
                    "ok": false,
                    "type": kind,
                    "error": format!("{e:#}"),
                }));
            } else {
                cli_eprintln!("App {app_id}: {e:#}");
            }
            failures.push((app_id, e));
        }
    }
    let Some((_, last)) = failures.last() else {
        return Ok(());
    };
    let kind = classify(last).kind;
    let summary = anyhow::anyhow!(TypedError::new(
        kind,
        format!("{} of {} resumes did not finish", failures.len(), ids.len()),
    ));
    if json {
        // Each failure printed its own line.
        return Err(anyhow::anyhow!(aurelia::core::error::Reported(summary)));
    }
    Err(summary)
}
