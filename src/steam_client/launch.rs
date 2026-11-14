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
pub fn select_launch_entry(
    launch_options: &[LaunchInfo],
    app: &LibraryGame,
    prefer_windows_target: bool,
    prefer_native_target: bool,
) -> Option<LaunchInfo> {
    let on_disk = |rel: &str| on_disk_path(app, rel);
    pick_launch_entry(
        launch_options,
        app,
        prefer_windows_target,
        prefer_native_target,
        &on_disk,
    )
    .map(|mut entry| {
        if let Some(exe) = on_disk(&entry.executable) {
            entry.executable = exe;
        }
        if let Some(wd) = entry
            .workingdir
            .as_deref()
            .filter(|wd| !wd.is_empty())
            .and_then(on_disk)
        {
            entry.workingdir = Some(wd);
        }
        entry
    })
}

/// Find a launch entry by id.
pub fn find_launch_option<'a>(options: &'a [LaunchInfo], id: &str) -> Result<&'a LaunchInfo> {
    options.iter().find(|o| o.id == id).ok_or_else(|| {
        let known: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
        anyhow!(
            "launch option {id} can't be started: it doesn't exist or is macOS-only (available: {})",
            known.join(", ")
        )
    })
}

/// Reject entries conflicting with explicit flags.
///
/// `required` names the demanded target and source.
pub fn check_launch_option_target(
    entry: &LaunchInfo,
    required: Option<(LaunchTarget, &str)>,
) -> Result<()> {
    let Some((target, source)) = required else {
        return Ok(());
    };
    if entry.target != target {
        let (build, needed) = match entry.target {
            LaunchTarget::NativeLinux => ("native Linux", "Windows"),
            LaunchTarget::WindowsProton => ("Windows", "native Linux"),
        };
        bail!(
            "launch option {} starts the {build} build, but {source} requires the {needed} build",
            entry.id
        );
    }
    Ok(())
}

/// Is this launch executable on disk?
pub fn launch_executable_installed(app: &LibraryGame, executable: &str) -> bool {
    app.is_installed && !executable.is_empty() && on_disk_path(app, executable).is_some()
}

/// Drop entries an installed game lacks.
pub fn hide_uninstalled_launch_options(options: &mut Vec<LaunchOptionInfo>, app: &LibraryGame) {
    if app.is_installed {
        options.retain(|o| launch_executable_installed(app, &o.executable));
    }
}

/// Require the entry's executable on disk.
pub(super) fn require_launch_option_installed(entry: &LaunchInfo, app: &LibraryGame) -> Result<()> {
    if !launch_executable_installed(app, &entry.executable) {
        bail!(
            "launch option {} needs {:?}, which isn't installed; see `aurelia launch-targets {}`",
            entry.id,
            entry.executable,
            app.app_id
        );
    }
    Ok(())
}

/// On-disk spelling of `rel`, ignoring case.
///
/// Windows manifests ignore path case.
fn on_disk_path(app: &LibraryGame, rel: &str) -> Option<String> {
    let dir = std::path::Path::new(app.install_path.as_deref()?);
    let found = crate::core::utils::resolve_path_ignore_case(dir, rel)?;
    let found = found
        .strip_prefix(dir)
        .map(|p| p.to_path_buf())
        .unwrap_or(found);
    Some(found.to_string_lossy().into_owned())
}

/// Is `target`'s executable installed?
fn target_installed(
    launch_options: &[LaunchInfo],
    app: &LibraryGame,
    target: LaunchTarget,
) -> bool {
    launch_options.iter().any(|o| {
        o.target == target && !o.executable.is_empty() && on_disk_path(app, &o.executable).is_some()
    })
}

/// One build plus Steam mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchTargetChoice {
    /// Stable id, e.g. `windows-host`.
    pub id: &'static str,
    /// `linux` or `windows`.
    pub platform: &'static str,
    /// `runtime`, `host` or `none`.
    pub steam_mode: Option<&'static str>,
    /// `aurelia play` flags selecting it.
    pub flags: Vec<String>,
    /// Launchable right now.
    pub available: bool,
    /// `build`, `steam-runtime` or `host-steam`.
    pub missing: Option<&'static str>,
    /// What plain `play` picks.
    pub default: bool,
}

/// List every build/Steam combination the game offers.
///
/// `default_entry` and `default_mode` mirror `play`.
pub fn launch_targets(
    launch_options: &[LaunchInfo],
    app: &LibraryGame,
    runtime_installed: bool,
    host_installed: bool,
    steam_flag: bool,
    default_entry: Option<LaunchTarget>,
    default_mode: crate::infra::runners::SteamMode,
) -> Vec<LaunchTargetChoice> {
    use crate::infra::runners::SteamMode;
    let flags = |extra: &[&str]| extra.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let offers = |target: LaunchTarget| launch_options.iter().any(|o| o.target == target);
    let mut targets = Vec::new();

    if offers(LaunchTarget::NativeLinux) {
        let installed = target_installed(launch_options, app, LaunchTarget::NativeLinux);
        let mut f = flags(&["--platform", "linux"]);
        if steam_flag {
            f.push("--steam".to_string());
        }
        targets.push(LaunchTargetChoice {
            id: "linux",
            platform: "linux",
            steam_mode: None,
            flags: f,
            available: installed,
            missing: (!installed).then_some("build"),
            default: installed && default_entry == Some(LaunchTarget::NativeLinux),
        });
    }

    if offers(LaunchTarget::WindowsProton) {
        let installed = target_installed(launch_options, app, LaunchTarget::WindowsProton);
        let default_windows = installed && default_entry == Some(LaunchTarget::WindowsProton);
        let modes = [
            (
                "windows-runtime",
                "runtime",
                SteamMode::InWineRuntime,
                (!runtime_installed).then_some("steam-runtime"),
            ),
            (
                "windows-host",
                "host",
                SteamMode::HostBridge,
                (!host_installed).then_some("host-steam"),
            ),
            ("windows-standalone", "none", SteamMode::Standalone, None),
        ];
        for (id, mode, steam_mode, steam_missing) in modes {
            // Missing build outranks missing Steam.
            let missing = if installed {
                steam_missing
            } else {
                Some("build")
            };
            targets.push(LaunchTargetChoice {
                id,
                platform: "windows",
                steam_mode: Some(mode),
                flags: flags(&["--platform", "windows", "--steam-mode", mode]),
                available: missing.is_none(),
                missing,
                default: default_windows && missing.is_none() && default_mode == steam_mode,
            });
        }
    }

    // Fallback default: first available.
    if !targets.iter().any(|t| t.default) {
        if let Some(first) = targets.iter_mut().find(|t| t.available) {
            first.default = true;
        }
    }
    targets
}

fn pick_launch_entry(
    launch_options: &[LaunchInfo],
    app: &LibraryGame,
    prefer_windows_target: bool,
    prefer_native_target: bool,
    on_disk: &dyn Fn(&str) -> Option<String>,
) -> Option<LaunchInfo> {
    let exe_exists =
        |o: &LaunchInfo| -> bool { !o.executable.is_empty() && on_disk(&o.executable).is_some() };
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
            .or_else(|| {
                launch_options
                    .iter()
                    .find(|o| o.target == LaunchTarget::WindowsProton)
            })
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
        launch_option: Option<&str>,
        on_target_resolved: Option<&(dyn Fn(Option<&str>) + Send + Sync)>,
        shaders: crate::launch::shaders::ShaderLaunch,
    ) -> Result<LaunchInfo> {
        // A Family-Shared game (licensed to another account) can only be authorised
        // by a running Steam client, so it always needs Steam integration regardless
        // of the user's preference.
        let mut steam_enabled =
            crate::launch::stand_in_guard::wants_valve_steam(steam_enabled, app.is_owned, false);

        // Reject bad entries before starting Steam.
        let (launch_options, category_ids) =
            self.get_product_info_and_categories(app.app_id).await?;
        let launch_options = match launch_option {
            Some(id) => {
                let entry = find_launch_option(&launch_options, id)?;
                require_launch_option_installed(entry, app)?;
                vec![entry.clone()]
            }
            None => launch_options,
        };

        let mut launcher_config = load_launcher_config().await?;
        // Auto picks per launch.
        let choice = crate::launch::stand_in_auto::apply(
            &mut launcher_config,
            crate::core::utils::is_steam_running,
            crate::launch::stand_in_auto::installed,
        );
        if let Some(note) = choice.note() {
            tracing::warn!("{note}");
        }
        guard_stand_in(
            app,
            &mut launcher_config,
            category_ids.as_deref(),
            force_native_engine,
        )?;
        // Stand-in replaces every Steam client.
        let stand_in = crate::launch::stand_in_guard::stand_in_applies(
            &launcher_config,
            app.app_id,
            force_native_engine,
        );
        if stand_in && steam_enabled {
            tracing::warn!("{}", crate::launch::stand_in_guard::REPLACES_STEAM_NOTE);
        }
        steam_enabled =
            crate::launch::stand_in_guard::wants_valve_steam(steam_enabled, true, stand_in);

        // With Steam integration the game talks to the host Steam client; make sure
        // one is running (start it silently if not) so Steamworks/Family-Sharing can
        // initialise. Best-effort.
        #[cfg(target_os = "windows")]
        if steam_enabled {
            SteamClient::ensure_host_steam_ready(std::time::Duration::from_secs(30)).await;
        }
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

        let prefer_windows_target = force_windows || proton_path.is_some();
        let launch_info = select_launch_entry(
            &launch_options,
            app,
            prefer_windows_target,
            prefer_native_target,
        )
        .ok_or_else(|| anyhow!("no launch options"))?;

        // Proton/Wine only exists on Linux. On Windows, a Windows game runs natively, so
        // run its executable directly instead of routing through the Proton pipeline.
        let native_windows = force_windows
            || (cfg!(target_os = "windows") && launch_info.target == LaunchTarget::WindowsProton);

        match launch_info.target {
            _ if native_windows => {
                tracing::info!(
                    app_id = app.app_id,
                    "resolved launch platform: Windows (native)"
                );
            }
            LaunchTarget::NativeLinux => {
                tracing::info!(
                    app_id = app.app_id,
                    "resolved launch platform: native Linux"
                );
            }
            LaunchTarget::WindowsProton => {
                tracing::info!(
                    app_id = app.app_id,
                    proton = proton_path.unwrap_or(&launcher_config.proton_version),
                    "resolved launch platform: Windows via Proton"
                );
            }
        }

        // None = native (Linux, or Windows without Proton); Some = the Proton runner.
        if let Some(cb) = on_target_resolved {
            cb(match launch_info.target {
                LaunchTarget::WindowsProton if !native_windows => {
                    Some(proton_path.unwrap_or(&launcher_config.proton_version))
                }
                _ => None,
            });
        }

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

        if chosen_proton_path.is_some() && launcher_config.anticheat_auto_install {
            self.ensure_anticheat_runtimes(app).await;
        }

        let cloud_enabled = launcher_config.enable_cloud_sync && !self.is_offline();
        let mut cloud_client = None;
        let mut cloud_resolver = None;
        let mut cloud_specs: Vec<crate::library::cloud_sync::UfsSaveSpec> = Vec::new();

        if cloud_enabled {
            let client = CloudClient::new(self.require_connection_owned()?);
            let remote_root = default_cloud_root(client.steam_id(), app.app_id)?.join("remote");
            // `%Win*%` cloud roots point inside the game's Proton prefix; without it
            // every Auto-Cloud save of a Windows game would be silently skipped.
            let cloud_user_configs = crate::core::config::load_user_configs().await?;
            // UFS rules and overrides; best-effort.
            let ufs = self.fetch_ufs_config(app.app_id).await.unwrap_or_default();
            let runs_windows = native_windows || chosen_proton_path.is_some();
            let target_os = if runs_windows { "windows" } else { "linux" };
            // Native Linux builds have no prefix.
            let wine_prefix = chosen_proton_path.is_some().then(|| {
                crate::core::utils::game_save_prefix(
                    &launcher_config,
                    app.app_id,
                    &cloud_user_configs,
                    app.install_path.as_deref().map(std::path::Path::new),
                )
            });
            let resolver =
                CloudPathResolver::new(remote_root, app.install_path.as_ref().map(PathBuf::from))
                    .with_wine_prefix(wine_prefix)
                    .with_root_overrides(&ufs.root_overrides, target_os)
                    .with_steam_id(client.steam_id())
                    .with_remote_uploads(super::remote_storage::stand_in_cloud_writes(
                        &launcher_config,
                    ));
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
            // Lets sync_up discover new local saves.
            cloud_client = Some(client);
            cloud_resolver = Some(resolver);
            cloud_specs = ufs.savefiles;
        }

        let (mut child, stand_in_log) = if native_windows {
            (
                self.spawn_windows_native(app, &launch_info, user_config)
                    .await?,
                None,
            )
        } else {
            let (child, log) = self
                .spawn_game_process(
                    app,
                    &launch_info,
                    chosen_proton_path,
                    &launcher_config,
                    user_config,
                    force_native_engine,
                    force_umu,
                    launch_script_override,
                    disable_launch_script,
                    steam_enabled,
                    shaders,
                )
                .await?;
            (child.into(), log)
        };

        // Record the launch so a separate `aurelia stop <app_id>` invocation can
        // find and terminate the process while we block on `wait()` below.
        let wineprefix = if native_windows {
            None
        } else {
            let user_configs = crate::core::config::load_user_configs().await?;
            let pfx = crate::core::utils::steam_wineprefix_for_game(
                &launcher_config,
                app.app_id,
                &user_configs,
                app.install_path.as_deref().map(std::path::Path::new),
            );
            // Only record a per-game (compatdata) prefix — sweeping the shared
            // master prefix on stop would also kill the Steam client inside it.
            pfx.to_string_lossy().contains("compatdata").then_some(pfx)
        };
        let record = crate::compat::running::RunningGame::new(
            app.app_id,
            app.name.clone(),
            child.id(),
            wineprefix,
        );
        if let Err(e) = crate::compat::running::record_launch(&record) {
            tracing::warn!(appid = app.app_id, "could not record running game: {e:#}");
        }

        // On the blocking pool: `Child::wait` blocks its thread for the game's whole
        // run, and on a runtime worker that starves the tasks tied to it. In the
        // daemon that froze the shared Steam connection and every other forwarded
        // command (`running`, `stop`) until the game exited.
        let wait_result = tokio::task::spawn_blocking(move || child.wait())
            .await
            .context("game wait task failed")?
            .context("failed waiting for game process exit");
        // Wrappers exit while the game lives on; wait for the app's processes.
        loop {
            let survivors = crate::compat::running::processes_for_app(app.app_id);
            let Some(&pid) = survivors.first() else { break };
            let mut record = record.clone();
            record.set_pid(pid);
            let _ = crate::compat::running::record_launch(&record);
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        crate::compat::running::clear(app.app_id);
        report_stand_in_log(stand_in_log.as_deref());
        wait_result?;

        if cloud_enabled {
            if let (Some(client), Some(resolver)) = (cloud_client.as_ref(), cloud_resolver.as_ref())
            {
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
                        tracing::warn!(
                            appid = app.app_id,
                            "Cloud upload failed (continuing): {e:#}"
                        )
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

            emit_queued(
                &tx,
                if verify_mode {
                    "verifying installed chunks"
                } else {
                    "resolving latest manifest"
                },
            )
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

            // Hosts first: fail before reporter spawns.
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
            emit_completed(
                &tx,
                if verify_mode {
                    "verify completed"
                } else {
                    "update completed"
                },
            )
            .await;
        });

        Ok(rx)
    }
}

/// Refuse protected games in stand-in mode.
fn guard_stand_in(
    app: &LibraryGame,
    config: &mut crate::core::config::LauncherConfig,
    category_ids: Option<&[u32]>,
    force_native_engine: bool,
) -> Result<()> {
    use crate::launch::stand_in_guard::{self as guard, Verdict};
    if !guard::stand_in_applies(config, app.app_id, force_native_engine) {
        return Ok(());
    }
    let anticheats = app
        .install_path
        .as_deref()
        .map(|p| crate::compat::anticheat::detect(Path::new(p)))
        .unwrap_or_default();
    let found = guard::protections(category_ids, &anticheats);
    match guard::evaluate(app.app_id, found, config) {
        Verdict::Clear => Ok(()),
        Verdict::Allowed(found) => {
            tracing::warn!("{}", guard::allowed_warning(app.app_id, &app.name, &found));
            Ok(())
        }
        // Auto: normal launch instead.
        Verdict::Refused(found) if config.steamworks_client_auto => {
            config.steamworks_client = false;
            let labels = guard::labels(&found);
            let note =
                crate::launch::stand_in_auto::refused_fallback_note(app.app_id, &app.name, &labels);
            tracing::warn!("{note}");
            Ok(())
        }
        Verdict::Refused(found) => Err(crate::core::error::TypedError::new(
            crate::core::error::ErrorKind::AccessDenied,
            guard::refusal_message(app.app_id, &app.name, &found),
        )
        .into()),
    }
}

/// Surface the stand-in log summary.
fn report_stand_in_log(log: Option<&Path>) {
    #[cfg(target_os = "linux")]
    if let Some(log) = log {
        for line in crate::launch::steamworks_overlay::summarize_log_file(log) {
            tracing::warn!("{line}");
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = log;
}
