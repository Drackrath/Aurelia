//! `SteamClient` methods: play/launch, update/verify, download driver.
//!
//! Split out of `steam_client.rs` for readability; the struct, shared imports
//! and free helpers live in the parent module (in scope via `use super::*`).
use super::*;

/// Pick the launch entry to run.
///
/// Only one platform's depot is installed, so a launch entry is only usable if its
/// executable is actually on disk. A game commonly advertises Windows, macOS and
/// Linux entries; picking one by order alone can select a build that was never
/// installed. After a platform switch, files from the previous platform may also
/// still be on disk, so "executable exists" alone is not enough either — prefer
/// the entry matching the *installed* platform, then any entry whose executable
/// exists, then the declared order.
///
/// With `prefer_windows_target` (a native Windows launch or an explicit/configured
/// Proton runner), the Windows entry wins whenever its executable is installed —
/// but a Proton request can't override which depot is actually on disk, so a
/// native-Linux-only install still runs its native build.
pub(crate) fn select_launch_entry(
    launch_options: &[LaunchInfo],
    app: &LibraryGame,
    prefer_windows_target: bool,
    prefer_native_target: bool,
) -> Option<LaunchInfo> {
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
    let installed_target = app.platform.as_deref().and_then(|p| match p {
        "linux" => Some(LaunchTarget::NativeLinux),
        "windows" => Some(LaunchTarget::WindowsProton),
        _ => None,
    });

    // Explicit per-game preference beats the manifest.
    if prefer_native_target {
        if let Some(entry) = launch_options
            .iter()
            .find(|o| o.target == LaunchTarget::NativeLinux && exe_exists(o))
        {
            return Some(entry.clone());
        }
    }

    if prefer_windows_target {
        launch_options
            .iter()
            .find(|o| o.target == LaunchTarget::WindowsProton && exe_exists(o))
            .or_else(|| {
                launch_options
                    .iter()
                    .find(|o| installed_target.is_some_and(|t| o.target == t) && exe_exists(o))
            })
            .or_else(|| launch_options.iter().find(|o| exe_exists(o)))
            .or_else(|| launch_options.iter().find(|o| o.target == LaunchTarget::WindowsProton))
            .or_else(|| launch_options.first())
            .cloned()
    } else {
        launch_options
            .iter()
            .find(|o| installed_target.is_some_and(|t| o.target == t) && exe_exists(o))
            .or_else(|| launch_options.iter().find(|o| exe_exists(o)))
            .or_else(|| launch_options.first())
            .cloned()
    }
}

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
        prefer_native_target: bool,
        on_target_resolved: Option<&(dyn Fn(Option<&str>) + Send + Sync)>,
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
            // Avoid racing a cold client.
            let timeout = std::time::Duration::from_secs(30);
            if crate::core::utils::wait_for_steam_logged_on(timeout).await {
                tracing::info!("host Steam logged on; proceeding with launch");
            } else {
                tracing::warn!(
                    "host Steam not logged on within {}s; launching anyway (Steamworks auth may fail)",
                    timeout.as_secs()
                );
            }
        }

        let launch_options = self.get_product_info(app.app_id).await?;

        let prefer_windows_target = force_windows || proton_path.is_some();
        let launch_info =
            select_launch_entry(&launch_options, app, prefer_windows_target, prefer_native_target)
                .ok_or_else(|| anyhow!("no launch options"))?;

        let launcher_config = load_launcher_config().await?;

        match launch_info.target {
            LaunchTarget::NativeLinux => {
                tracing::info!(app_id = app.app_id, "resolved launch platform: native Linux");
            }
            LaunchTarget::WindowsProton => {
                tracing::info!(
                    app_id = app.app_id,
                    proton = proton_path.unwrap_or(&launcher_config.proton_version),
                    "resolved launch platform: Windows via Proton"
                );
            }
        }
        // None = native Linux; Some = the effective Proton runner.
        if let Some(cb) = on_target_resolved {
            cb(match launch_info.target {
                LaunchTarget::NativeLinux => None,
                LaunchTarget::WindowsProton => {
                    Some(proton_path.unwrap_or(&launcher_config.proton_version))
                }
            });
        }

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
                self.require_connection_owned()?,
            );
            let remote_root = default_cloud_root(client.steam_id(), app.app_id)?.join("remote");
            // `%Win*%` cloud roots point inside the game's Proton prefix; without it
            // every Auto-Cloud save of a Windows game would be silently skipped.
            let cloud_user_configs = crate::core::config::load_user_configs().await?;
            let resolver = CloudPathResolver::new(
                remote_root,
                app.install_path.as_ref().map(PathBuf::from),
            )
            .with_wine_prefix(Some(crate::core::utils::game_save_prefix(
                &launcher_config,
                app.app_id,
                &cloud_user_configs,
            )));
            tracing::info!(appid = app.app_id, "Syncing Cloud...");
            // Conflict-safe: a divergent save is left untouched (never clobbered),
            // so the user can resolve it via `cloud sync` / the Heroic chooser. The
            // game launches with whatever is currently on disk.
            match client.sync_down(app.app_id, &resolver).await {
                Ok(outcome) => {
                    if outcome.has_conflicts() {
                        tracing::warn!(
                            appid = app.app_id,
                            "{} Cloud save(s) diverged from local — left untouched; resolve with `aurelia cloud sync`",
                            outcome.conflicts.len()
                        );
                    }
                    // Never let this pass unremarked: the game is about to start
                    // with saves missing that the user believes were synced.
                    if outcome.has_skips() {
                        tracing::warn!(
                            appid = app.app_id,
                            "{} Cloud save(s) could not be placed on disk (unmapped root token(s): {}) — the game may start without them; see `aurelia cloud sync {}`",
                            outcome.skipped.len(),
                            outcome
                                .skipped_tokens()
                                .iter()
                                .map(|t| format!("%{t}%"))
                                .collect::<Vec<_>>()
                                .join(", "),
                            app.app_id
                        );
                    }
                    // The worst case for the player: enough files arrived that the
                    // game starts, but not the ones holding their progress.
                    if outcome.has_failures() {
                        tracing::error!(
                            appid = app.app_id,
                            "{} Cloud save(s) FAILED to download — the save set on disk is incomplete and the game may show no progress. Run `aurelia cloud sync {} --down` before playing. First error: {}",
                            outcome.failed.len(),
                            app.app_id,
                            outcome.failed.first().map_or("", |f| f.error.as_str())
                        );
                    }
                }
                Err(e) => tracing::error!(
                    appid = app.app_id,
                    "Cloud sync-down failed, launching anyway — saves may be missing or incomplete: {e:#}"
                ),
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
            let user_configs = crate::core::config::load_user_configs().await?;
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
        // Wrappers exit while the game lives on; wait for the app's processes.
        loop {
            let survivors = crate::compat::running::processes_for_app(app.app_id);
            let Some(&pid) = survivors.first() else { break };
            let mut record = record.clone();
            record.pid = pid;
            let _ = crate::compat::running::record_launch(&record);
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
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
        let connection = self.require_connection_owned()?;

        let install_root = self.install_root_for_app(appid).await?;
        let manifest_path = self.appmanifest_path(appid).await?;
        let (tx, rx) = tokio::sync::mpsc::channel(128);

        let (local_manifests, active_branch) = self
            .local_manifest_info_for_appid(appid)
            .await
            .unwrap_or_else(|_| (HashMap::new(), "public".to_string()));

        let client_clone = self.clone();
        let shared_state_clone = shared_state.clone();
        // The name Steam already recorded for this copy.
        let recorded_name = std::fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|raw| parse_name_from_acf(&raw))
            .filter(|n| !n.starts_with("App "));
        let game_name = match recorded_name.clone() {
            Some(name) => name,
            None => self.resolve_install_game_name(appid).await,
        };
        tokio::task::spawn(async move {
            // `on_manifest` fills the total per-depot.
            if let Ok(mut state) = shared_state_clone.write() {
                state.begin(
                    appid,
                    game_name.clone(),
                    0,
                    format!("Preparing operation for {}...", game_name),
                );
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
                let mut remote =
                    SteamClient::remote_manifest_ids_static(&connection, appid, &active_branch)
                        .await
                        .unwrap_or_default();
                // An update refreshes only installed depots
                if !local_manifests.is_empty() {
                    remote.retain(|depot_id, _| local_manifests.contains_key(depot_id));
                }
                remote
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
                emit_failed(&tx, message).await;
                return;
            };

            // Hosts first, so a fetch failure emits no trailing frames.
            let Some(hosts) = fetch_content_hosts(&client_clone, &connection, &tx, appid).await
            else {
                if let Ok(mut state) = shared_state_clone.write() {
                    state.finish("Operation failed or paused");
                }
                return;
            };

            // Periodically forward the live byte counters over the channel.
            spawn_progress_reporter(
                tx.clone(),
                shared_state_clone.clone(),
                if verify_mode {
                    DownloadProgressState::Verifying
                } else {
                    DownloadProgressState::Downloading
                },
            );

            // Zero grand total: per-depot accumulation.
            let Some(successful_depots) = run_depot_loop(
                &client_clone,
                &connection,
                &tx,
                &shared_state_clone,
                appid,
                selections,
                &install_root,
                &hosts,
                &DepotLoopOpts {
                    verify_mode,
                    grand_total_bytes: 0,
                    manifest_overrides: None,
                },
            )
            .await
            else {
                if let Ok(mut state) = shared_state_clone.write() {
                    state.finish("Operation failed or paused");
                }
                return;
            };

            if let Ok(mut state) = shared_state_clone.write() {
                state.is_downloading = false;
                state.status_text = "Operation complete".to_string();
            }

            // The content was written into `install_root`
            let game_name = match recorded_name {
                Some(name) => name,
                None => client_clone.resolve_install_game_name(appid).await,
            };
            let installdir = install_root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| sanitize_install_dir(&game_name));
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
            emit_completed(&tx, if verify_mode { "verify completed" } else { "update completed" })
                .await;
        });

        Ok(rx)
    }

}
