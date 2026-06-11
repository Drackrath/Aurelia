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
        user_config: Option<&crate::models::UserAppConfig>,
        force_windows: bool,
    ) -> Result<LaunchInfo> {
        let prefer_proton = force_windows || proton_path.is_some();
        let launch_options = self.get_product_info(app.app_id, prefer_proton).await?;
        // When forcing a Windows launch, prefer a Windows executable entry.
        let launch_info = if force_windows {
            launch_options
                .iter()
                .find(|o| o.target == LaunchTarget::WindowsProton)
                .or_else(|| launch_options.first())
                .cloned()
        } else {
            launch_options.first().cloned()
        }
        .ok_or_else(|| anyhow!("no launch options"))?;

        let launcher_config = load_launcher_config().await.unwrap_or_default();

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
        let mut local_root = None;

        if cloud_enabled {
            let client = CloudClient::new(
                self.connection
                    .as_ref()
                    .cloned()
                    .context("steam connection not initialized")?,
            );
            let root = default_cloud_root(client.steam_id(), app.app_id)?;
            tracing::info!(appid = app.app_id, path = %root.display(), "Syncing Cloud...");
            let _ = client.sync_down(app.app_id, &root).await;
            cloud_client = Some(client);
            local_root = Some(root);
        }

        let mut child = if native_windows {
            self.spawn_windows_native(app, &launch_info, user_config).await?
        } else {
            self.spawn_game_process(app, &launch_info, chosen_proton_path, &launcher_config, user_config).await?
        };

        // Record the launch so a separate `aurelia stop <app_id>` invocation can
        // find and terminate the process while we block on `wait()` below.
        let wineprefix = if native_windows {
            None
        } else {
            let user_configs = crate::config::load_user_configs().await.unwrap_or_default();
            let pfx = crate::utils::steam_wineprefix_for_game(&launcher_config, app.app_id, &user_configs);
            // Only record a per-game (compatdata) prefix — sweeping the shared
            // master prefix on stop would also kill the Steam client inside it.
            pfx.to_string_lossy().contains("compatdata").then_some(pfx)
        };
        let record = crate::running::RunningGame {
            app_id: app.app_id,
            name: app.name.clone(),
            pid: child.id(),
            wineprefix,
        };
        if let Err(e) = crate::running::record_launch(&record) {
            tracing::warn!(appid = app.app_id, "could not record running game: {e:#}");
        }

        let wait_result = child.wait().context("failed waiting for game process exit");
        crate::running::clear(app.app_id);
        wait_result?;

        if cloud_enabled {
            if let (Some(client), Some(root)) = (cloud_client.as_ref(), local_root.as_ref()) {
                // The game has already run and exited, so a cloud-upload failure must not
                // be surfaced as a launch failure. Log it and continue (this mirrors the
                // best-effort sync_down before launch).
                match client.sync_up(app.app_id, root).await {
                    Ok(()) => tracing::info!(appid = app.app_id, "Upload Complete"),
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
        user_config: Option<&crate::models::UserAppConfig>,
    ) -> Result<()> {
        let launcher_config = load_launcher_config().await.unwrap_or_default();
        self.spawn_game_process(app, launch_info, proton_path, &launcher_config, user_config).await?;
        Ok(())
    }

    pub async fn update_game(
        &self,
        appid: u32,
        shared_state: Arc<std::sync::RwLock<crate::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        self.start_manifest_download(appid, false, shared_state)
            .await
    }

    pub async fn verify_game(
        &self,
        appid: u32,
        shared_state: Arc<std::sync::RwLock<crate::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        self.start_manifest_download(appid, true, shared_state)
            .await
    }

    pub(crate) async fn start_manifest_download(
        &self,
        appid: u32,
        verify_mode: bool,
        shared_state: Arc<std::sync::RwLock<crate::models::DownloadState>>,
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

            let mut selections = Vec::new();
            for (depot_id, manifest_id) in &remote_manifests {
                selections.push(ManifestSelection {
                    app_id: appid,
                    depot_id: *depot_id as u32,
                    manifest_id: *manifest_id,
                    appinfo_vdf: String::new(),
                });
            }

            if selections.is_empty() {
                let _ = tx
                    .send(DownloadProgress {
                        state: DownloadProgressState::Failed,
                        current_file: "no manifest/depot available for download".to_string(),
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

            for selection in selections {
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

                    let (host_name, port) = if let Some(pos) = host.find(':') {
                        (
                            &host[..pos],
                            host[pos + 1..].parse::<u16>().unwrap_or(80),
                        )
                    } else {
                        (host.as_str(), 80)
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

                    let tx_clone = tx.clone();
                    let selection_depot_id = selection.depot_id;
                    let on_progress = Arc::new(move |bytes: u64| {
                        let _ = tx_clone.try_send(DownloadProgress {
                            state: if verify_mode {
                                DownloadProgressState::Verifying
                            } else {
                                DownloadProgressState::Downloading
                            },
                            bytes_downloaded: bytes,
                            depot_id: selection_depot_id,
                            depot_bytes_downloaded: bytes,
                            current_file: format!("Depot {}", selection_depot_id),
                            ..Default::default()
                        });
                    });

                    let depot_size = Arc::new(std::sync::atomic::AtomicU64::new(0));
                    let size_clone = depot_size.clone();
                    let on_manifest = Arc::new(move |total_bytes: u64| {
                        size_clone.store(total_bytes, std::sync::atomic::Ordering::SeqCst);
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
                            let aborted = shared_state_clone.read()
                                .map(|s| s.abort_signal.load(std::sync::atomic::Ordering::Relaxed))
                                .unwrap_or(false);
                            if aborted {
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
                    let aborted = shared_state_clone.read()
                        .map(|s| s.abort_signal.load(std::sync::atomic::Ordering::Relaxed))
                        .unwrap_or(false);

                    if aborted {
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
            } else {
                if let Ok(mut state) = shared_state_clone.write() {
                    state.is_downloading = false;
                    state.status_text = "Operation failed or paused".to_string();
                }
            }
        });

        Ok(rx)
    }

}
