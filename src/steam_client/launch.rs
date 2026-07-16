//! `SteamClient` methods: play/launch, update/verify, download driver.
//!
//! Split out of `steam_client.rs` for readability; the struct, shared imports
//! and free helpers live in the parent module (in scope via `use super::*`).
use super::*;

impl SteamClient {
    pub async fn play_game(
        &mut self,
        app: &LibraryGame,
        proton_path: Option<&str>,
        user_config: Option<&crate::core::models::UserAppConfig>,
        force_windows: bool,
        force_native_engine: bool,
        force_umu: bool,
        launch_script_override: Option<PathBuf>,
        disable_launch_script: bool,
        steam_enabled: bool,
    ) -> Result<LaunchInfo> {
        // A Family-Shared game (licensed to another account) can only be authorised
        // by a running Steam client, so it always needs Steam integration regardless
        // of the user's preference.
        let steam_enabled = steam_enabled || !app.is_owned;

        // With Steam integration the game talks to the host Steam client; make sure
        // one is running (start it silently if not) so Steamworks/Family-Sharing can
        // initialise. Best-effort and Linux-only.
        #[cfg(target_os = "linux")]
        if steam_enabled {
            crate::core::utils::ensure_steam_running();
        }

        let launch_options = self.get_product_info(app.app_id).await?;

        // Only one platform's depot is installed, so a launch entry is only usable
        // if its executable is actually on disk. A game commonly advertises Windows,
        // macOS and Linux entries; picking one by order alone can select a build that
        // was never installed (e.g. choosing the Windows `.exe` for a game where only
        // the native Linux depot is present), which then fails with
        // "game_executable_not_found". Prefer entries whose executable exists.
        let exe_exists = |o: &LaunchInfo| -> bool {
            match app.install_path.as_deref() {
                Some(dir) if !o.executable.is_empty() => {
                    std::path::Path::new(dir)
                        .join(o.executable.replace('\\', "/"))
                        .exists()
                }
                _ => false,
            }
        };

        // Prefer the Windows executable entry whenever we intend to run through a
        // Proton/Wine layer — either a native Windows launch (`force_windows`) or
        // an explicit/configured Proton runner (`proton_path`). `--proton` would
        // otherwise be ignored when a game's first entry is a macOS/Linux build.
        // Within each preference, an entry whose executable is installed wins over
        // one that isn't. With no runner and no force, pick the installed entry
        // (the platform whose depot is present), falling back to the first.
        let prefer_windows_target = force_windows || proton_path.is_some();
        let launch_info = if prefer_windows_target {
            launch_options
                .iter()
                .find(|o| o.target == LaunchTarget::WindowsProton && exe_exists(o))
                // No installed Windows build: a Proton request can't override which
                // depot is actually on disk. A native-Linux-only game (e.g. one a
                // driver like Heroic launches with a default `--proton`) must still
                // run its installed native build rather than a non-existent `.exe`.
                .or_else(|| launch_options.iter().find(|o| exe_exists(o)))
                .or_else(|| launch_options.iter().find(|o| o.target == LaunchTarget::WindowsProton))
                .or_else(|| launch_options.first())
                .cloned()
        } else {
            launch_options
                .iter()
                .find(|o| exe_exists(o))
                .or_else(|| launch_options.first())
                .cloned()
        }
        .ok_or_else(|| anyhow!("no launch options"))?;

        let launcher_config = load_launcher_config().await?;

        // Proton/Wine only exists on Linux. On Windows, a Windows game runs natively, so
        // run its executable directly instead of routing through the Proton pipeline.
        let native_windows = force_windows
            || (cfg!(target_os = "windows") && launch_info.target == LaunchTarget::WindowsProton);

        let chosen_proton_path = if native_windows {
            None
        } else {
            match launch_info.target {
                LaunchTarget::NativeLinux => None,
                LaunchTarget::WindowsProton => {
                    proton_path.or(Some(launcher_config.proton_version.as_str()))
                }
            }
        };

        let cloud_enabled = launcher_config.enable_cloud_sync && !self.is_offline();
        let mut cloud_client = None;
        let mut cloud_resolver = None;
        let mut cloud_specs: Vec<crate::library::cloud_sync::UfsSaveSpec> = Vec::new();

        if cloud_enabled {
            let client = CloudClient::new(
                self.connection
                    .as_ref()
                    .cloned()
                    .context("steam connection not initialized")?,
            );
            let remote_root = default_cloud_root(client.steam_id(), app.app_id)?.join("remote");
            let resolver = CloudPathResolver::new(
                remote_root,
                app.install_path.as_ref().map(PathBuf::from),
            );
            tracing::info!(appid = app.app_id, "Syncing Cloud...");
            // Conflict-safe: a divergent save is left untouched (never clobbered),
            // so the user can resolve it via `cloud sync` / the Heroic chooser. The
            // game launches with whatever is currently on disk.
            match client.sync_down(app.app_id, &resolver).await {
                Ok(outcome) if outcome.has_conflicts() => tracing::warn!(
                    appid = app.app_id,
                    "{} Cloud save(s) diverged from local — left untouched; resolve with `aurelia cloud sync`",
                    outcome.conflicts.len()
                ),
                Ok(_) => {}
                Err(e) => tracing::warn!(appid = app.app_id, "Cloud sync-down failed (continuing): {e:#}"),
            }
            // UFS rules let sync_up discover brand-new local saves; best-effort.
            let specs = self.fetch_ufs_save_specs(app.app_id).await.unwrap_or_default();
            cloud_client = Some(client);
            cloud_resolver = Some(resolver);
            cloud_specs = specs;
        }

        let mut child = if native_windows {
            self.spawn_windows_native(app, &launch_info, user_config).await?
        } else {
            self.spawn_game_process(app, &launch_info, chosen_proton_path, &launcher_config, user_config, force_native_engine, force_umu, launch_script_override, disable_launch_script, steam_enabled).await?
        };

        // Record the launch so a separate `aurelia stop <app_id>` invocation can
        // find and terminate the process while we block on `wait()` below.
        let wineprefix = if native_windows {
            None
        } else {
            let user_configs = crate::core::config::load_user_configs().await.unwrap_or_default();
            let pfx = crate::core::utils::steam_wineprefix_for_game(&launcher_config, app.app_id, &user_configs);
            // Only record a per-game (compatdata) prefix — sweeping the shared
            // master prefix on stop would also kill the Steam client inside it.
            pfx.to_string_lossy().contains("compatdata").then_some(pfx)
        };
        let record = crate::compat::running::RunningGame {
            app_id: app.app_id,
            name: app.name.clone(),
            pid: child.id(),
            wineprefix,
        };
        if let Err(e) = crate::compat::running::record_launch(&record) {
            tracing::warn!(appid = app.app_id, "could not record running game: {e:#}");
        }

        let wait_result = child.wait().context("failed waiting for game process exit");
        crate::compat::running::clear(app.app_id);
        wait_result?;

        if cloud_enabled {
            if let (Some(client), Some(resolver)) = (cloud_client.as_ref(), cloud_resolver.as_ref()) {
                // The game has already run and exited, so a cloud-upload failure must not
                // be surfaced as a launch failure. Log it and continue (this mirrors the
                // best-effort sync_down before launch).
                match client.sync_up(app.app_id, resolver, &cloud_specs).await {
                    Ok(outcome) if outcome.has_conflicts() => tracing::warn!(
                        appid = app.app_id,
                        "{} Cloud save(s) diverged on upload — left untouched; resolve with `aurelia cloud sync`",
                        outcome.conflicts.len()
                    ),
                    Ok(_) => tracing::info!(appid = app.app_id, "Upload Complete"),
                    Err(e) => {
                        tracing::warn!(appid = app.app_id, "Cloud upload failed (continuing): {e:#}")
                    }
                }
            }
        }

        Ok(launch_info)
    }

    pub async fn launch_game(
        &self,
        app: &LibraryGame,
        launch_info: &LaunchInfo,
        proton_path: Option<&str>,
        user_config: Option<&crate::core::models::UserAppConfig>,
    ) -> Result<()> {
        let launcher_config = load_launcher_config().await?;
        self.spawn_game_process(app, launch_info, proton_path, &launcher_config, user_config, false, false, None, false, false).await?;
        Ok(())
    }

    pub async fn update_game(
        &self,
        appid: u32,
        shared_state: Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        self.start_manifest_download(appid, false, shared_state)
            .await
    }

    pub async fn verify_game(
        &self,
        appid: u32,
        shared_state: Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        self.start_manifest_download(appid, true, shared_state)
            .await
    }

    pub(crate) async fn start_manifest_download(
        &self,
        appid: u32,
        verify_mode: bool,
        shared_state: Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        let connection = self
            .connection
            .as_ref()
            .cloned()
            .context("steam connection not initialized")?;

        let install_root = self.install_root_for_app(appid).await?;
        let manifest_path = self.appmanifest_path(appid).await?;
        let (tx, rx) = tokio::sync::mpsc::channel(128);

        let (local_manifests, active_branch) = self
            .local_manifest_info_for_appid(appid)
            .await
            .unwrap_or_else(|_| (HashMap::new(), "public".to_string()));

        let client_clone = self.clone();
        let shared_state_clone = shared_state.clone();
        let game_name = self.resolve_install_game_name(appid).await;
        tokio::task::spawn(async move {
            if let Ok(mut state) = shared_state_clone.write() {
                state.is_downloading = true;
                state.is_paused = false;
                state.app_id = appid;
                state.app_name = game_name.clone();
                state.downloaded_bytes = 0;
                // Reset the byte counters the progress reporter reads so a previous
                // operation's totals don't leak in. The whole-app total has no PICS
                // pre-sum here; `on_manifest` fills it in as each depot is fetched.
                state.total_bytes = 0;
                state.depot_id = 0;
                state.depot_downloaded_bytes = 0;
                state.depot_total_bytes = 0;
                state.status_text = format!("Preparing operation for {}...", game_name);
            }

            let _ = tx
                .send(DownloadProgress {
                    state: DownloadProgressState::Queued,
                    current_file: if verify_mode {
                        "verifying installed chunks".to_string()
                    } else {
                        "resolving latest manifest".to_string()
                    },
                    ..Default::default()
                })
                .await;

            let remote_manifests = if verify_mode {
                local_manifests.clone()
            } else {
                SteamClient::remote_manifest_ids_static(&connection, appid, &active_branch)
                    .await
                    .unwrap_or_default()
            };

            let selections: Vec<ManifestSelection> = remote_manifests
                .iter()
                .map(|(depot_id, manifest_id)| ManifestSelection {
                    app_id: appid,
                    depot_id: *depot_id as u32,
                    manifest_id: *manifest_id,
                    appinfo_vdf: String::new(),
                })
                .collect();

            if selections.is_empty() {
                // In verify mode the selections come from the local appmanifest's
                // `InstalledDepots`. Empty means the app isn't fully installed (e.g.
                // only staged/partially downloaded), which is otherwise reported with
                // a confusing "no manifest/depot available" — spell it out instead.
                let message = if verify_mode {
                    format!(
                        "app {appid} has no installed depots to verify — it is not fully \
                         installed (its appmanifest lists no completed depots, e.g. a \
                         staged or partial download). Run `aurelia install {appid}` to \
                         complete the installation."
                    )
                } else {
                    format!(
                        "no manifest/depot available for app {appid} (no downloadable \
                         depot was resolved for the active branch)"
                    )
                };
                let _ = tx
                    .send(DownloadProgress {
                        state: DownloadProgressState::Failed,
                        current_file: message,
                        ..Default::default()
                    })
                    .await;
                return;
            };

            let hosts = match client_clone.get_content_servers(connection.cell_id()).await {
                Ok(h) => h,
                Err(e) => {
                    let _ = tx
                        .send(DownloadProgress {
                            state: DownloadProgressState::Failed,
                            current_file: format!("Failed to fetch content servers: {}", e),
                            ..Default::default()
                        })
                        .await;
                    return;
                }
            };

            let mut success = true;
            let mut successful_depots = Vec::new();

            // Periodically forward the live byte counters over the channel.
            let progress_tx = tx.clone();
            let progress_state = shared_state_clone.clone();
            let report_verify_mode = verify_mode;
            tokio::spawn(async move {
                let mut ticker =
                    tokio::time::interval(std::time::Duration::from_millis(250));
                loop {
                    ticker.tick().await;
                    let snapshot = match progress_state.read() {
                        Ok(s) => Some((
                            s.is_downloading,
                            s.downloaded_bytes,
                            s.total_bytes,
                            s.status_text.clone(),
                            s.depot_id,
                            s.depot_downloaded_bytes,
                            s.depot_total_bytes,
                        )),
                        Err(_) => None,
                    };
                    let Some((
                        downloading,
                        downloaded,
                        total,
                        status,
                        depot_id,
                        depot_downloaded,
                        depot_total,
                    )) = snapshot
                    else {
                        break;
                    };
                    if !downloading {
                        break;
                    }
                    if progress_tx
                        .send(DownloadProgress {
                            state: if report_verify_mode {
                                DownloadProgressState::Verifying
                            } else {
                                DownloadProgressState::Downloading
                            },
                            bytes_downloaded: downloaded,
                            total_bytes: total,
                            current_file: status,
                            depot_id,
                            depot_bytes_downloaded: depot_downloaded,
                            depot_total_bytes: depot_total,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });

            for selection in selections {
                // Restart the per-depot counters so the current depot's progress is
                // reported from zero (the whole-app counters keep accumulating).
                if let Ok(mut state) = shared_state_clone.write() {
                    state.depot_id = selection.depot_id;
                    state.depot_downloaded_bytes = 0;
                    state.depot_total_bytes = 0;
                    state.status_text = if verify_mode {
                        format!("Verifying depot {}", selection.depot_id)
                    } else {
                        format!("Downloading depot {}", selection.depot_id)
                    };
                }

                let key: Vec<u8> = match client_clone.get_depot_key(appid, selection.depot_id).await {
                    Ok(k) => k,
                    Err(e) => {
                        tracing::warn!(
                            "Skipping Depot {} (No Key/Not Owned): {}",
                            selection.depot_id,
                            e
                        );
                        continue;
                    }
                };

                let manifest_code: Option<u64> = client_clone
                    .get_manifest_request_code(appid, selection.depot_id, selection.manifest_id)
                    .await
                    .ok();

                let mut depot_success = false;
                for host in &hosts {
                    let token: Option<String> = client_clone
                        .get_cdn_auth_token(appid, selection.depot_id, host)
                        .await
                        .ok();

                    let (host_name, port) = match host.split_once(':') {
                        Some((name, port)) => (name, port.parse::<u16>().unwrap_or(80)),
                        None => (host.as_str(), 80),
                    };

                    let cdn_server = steam_cdn::web_api::content_service::CDNServer {
                        r#type: "CDN".to_string(),
                        https: port == 443,
                        host: host_name.to_string(),
                        vhost: host_name.to_string(),
                        port,
                        cell_id: connection.cell_id(),
                        load: 0,
                        weighted_load: 0,
                        auth_token: token,
                    };

                    let cdn_client = steam_cdn::CDNClient::with_server(
                        Arc::new(connection.clone()),
                        cdn_server,
                    );

                    // Advance the cumulative byte counters 
                    let state_for_progress = shared_state_clone.clone();
                    let on_progress = Arc::new(move |bytes: u64| {
                        if let Ok(mut state) = state_for_progress.write() {
                            state.downloaded_bytes += bytes;
                            state.depot_downloaded_bytes += bytes;
                        }
                    });

                    let depot_size = Arc::new(std::sync::atomic::AtomicU64::new(0));
                    let size_clone = depot_size.clone();
                    let state_for_manifest = shared_state_clone.clone();
                    let on_manifest = Arc::new(move |total_bytes: u64| {
                        size_clone.store(total_bytes, std::sync::atomic::Ordering::SeqCst);
                        if let Ok(mut state) = state_for_manifest.write() {
                            // Accumulate per-depot totals
                            state.depot_total_bytes = total_bytes;
                            state.total_bytes += total_bytes;
                        }
                    });

                    let abort_signal = shared_state_clone
                        .read()
                        .ok()
                        .map(|s| s.abort_signal.clone());

                    match cdn_client
                        .download_depot(
                            appid,
                            selection.depot_id,
                            selection.manifest_id,
                            &key,
                            &install_root,
                            manifest_code,
                            verify_mode,
                            abort_signal,
                            Some(on_progress),
                            Some(on_manifest),
                        )
                        .await
                    {
                        Ok(_) => {
                            if download_aborted(&shared_state_clone) {
                                break;
                            }

                            depot_success = true;
                            successful_depots.push((
                                selection.depot_id,
                                selection.manifest_id,
                                depot_size.load(std::sync::atomic::Ordering::SeqCst),
                            ));
                            break;
                        }
                        Err(e) => {
                            tracing::error!("CDN Error from {}: {}", host, e);
                        }
                    }
                }

                if !depot_success {
                    if download_aborted(&shared_state_clone) {
                        success = false;
                        break;
                    }

                    let _ = tx
                        .send(DownloadProgress {
                            state: DownloadProgressState::Failed,
                            current_file: format!(
                                "Failed to download/verify depot {} from all servers",
                                selection.depot_id
                            ),
                            ..Default::default()
                        })
                        .await;
                    success = false;
                    break;
                }
            }

            if success {
                if let Ok(mut state) = shared_state_clone.write() {
                    state.is_downloading = false;
                    state.status_text = "Operation complete".to_string();
                }

                let (game_name, pics_installdir) = client_clone.resolve_install_game_info(appid).await;
                let installdir = pics_installdir.unwrap_or_else(|| sanitize_install_dir(&game_name));
                // Record the current build so Steam sees the install as up to date.
                let build_id =
                    SteamClient::remote_buildid_static(&connection, appid, &active_branch).await;

                if let Err(err) = SteamClient::write_appmanifest(
                    &manifest_path,
                    appid,
                    &game_name,
                    &installdir,
                    successful_depots,
                    build_id.as_deref(),
                    true,
                    false,
                ) {
                    tracing::warn!("failed writing appmanifest for {}: {}", appid, err);
                }
                let _ = tx
                    .send(DownloadProgress {
                        state: DownloadProgressState::Completed,
                        bytes_downloaded: 1,
                        total_bytes: 1,
                        current_file: if verify_mode {
                            "verify completed".to_string()
                        } else {
                            "update completed".to_string()
                        },
                        ..Default::default()
                    })
                    .await;
            } else if let Ok(mut state) = shared_state_clone.write() {
                state.is_downloading = false;
                state.status_text = "Operation failed or paused".to_string();
            }
        });

        Ok(rx)
    }

}

/// Returns whether the user has signalled an abort for the in-progress
/// download/verify. A poisoned lock is treated as "not aborted" so a transient
/// lock failure can't spuriously cancel the operation.
fn download_aborted(state: &Arc<std::sync::RwLock<crate::core::models::DownloadState>>) -> bool {
    state
        .read()
        .map(|s| s.abort_signal.load(std::sync::atomic::Ordering::Relaxed))
        .unwrap_or(false)
}
