//! `SteamClient` methods: branches, platform detection, install.
//!
//! Split out of `steam_client.rs` for readability; the struct, shared imports
//! and free helpers live in the parent module (in scope via `use super::*`).
use super::*;

/// Locate the `depots` object within a parsed appinfo VDF root object.
///
/// When the VDF root key is the well-known `appinfo`/`<appid>` wrapper, the
/// `depots` node sits directly under the root; otherwise it may be nested one
/// level down inside the first child object. Shared by `fetch_branches` and
/// `get_available_platforms`, which locate depots identically.
fn locate_depots<'a, 'text>(
    root_obj: &'a steam_vdf_parser::Obj<'text>,
    vdf_key: &str,
    appid: u32,
) -> Option<&'a steam_vdf_parser::Value<'text>> {
    if vdf_key == "appinfo" || vdf_key == appid.to_string() {
        root_obj.get("depots")
    } else {
        root_obj.get("depots").or_else(|| {
            root_obj
                .values()
                .next()
                .and_then(|v| v.as_obj())
                .and_then(|o| o.get("depots"))
        })
    }
}

impl SteamClient {
    /// Issue a single PICS `ProductInfo` request for `appid` and return the raw
    /// appinfo VDF buffer for that app. Shared by `fetch_branches` and
    /// `get_available_platforms`, which both need exactly this round-trip.
    ///
    /// `ctx` is folded into the request-failure error message so callers keep
    /// their original, distinct context strings.
    async fn fetch_appinfo_buffer(&self, appid: u32, ctx: &'static str) -> Result<Vec<u8>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut request = CMsgClientPICSProductInfoRequest::new();
        request
            .apps
            .push(cmsg_client_picsproduct_info_request::AppInfo {
                appid: Some(appid),
                ..Default::default()
            });

        let response: CMsgClientPICSProductInfoResponse =
            connection.job(request).await.context(ctx)?;

        let app = response
            .apps
            .iter()
            .find(|entry| entry.appid() == appid)
            .ok_or_else(|| anyhow!("missing app info payload for app {appid}"))?;

        Ok(app.buffer().to_vec())
    }

    pub async fn fetch_branches(&self, appid: u32) -> Result<Vec<String>> {
        // PICS returns the appinfo as *binary* VDF; parse that first and only fall
        // back to text (mirroring `get_available_platforms`). Parsing the binary
        // buffer as text — as this used to — fails with "Expected a valid token for
        // object start", which is why `branches` was broken.
        let buffer = self
            .fetch_appinfo_buffer(appid, "failed requesting appinfo product info for branches")
            .await?;
        let appinfo_vdf_text = String::from_utf8_lossy(&buffer);
        let vdf = steam_vdf_parser::parse_binary(&buffer)
            .or_else(|_| steam_vdf_parser::parse_text(&appinfo_vdf_text).map(|v| v.into_owned()))
            .context("failed parsing appinfo VDF")?;

        let root_obj = vdf.as_obj().context("appinfo VDF root is not an object")?;
        let depots = locate_depots(root_obj, vdf.key(), appid);

        let mut names: Vec<String> = Vec::new();
        if let Some(branches) = depots
            .and_then(|v| v.as_obj())
            .and_then(|d| d.get("branches"))
            .and_then(|b| b.as_obj())
        {
            for (name, node) in branches.iter() {
                // Skip private (password-protected) branches.
                let private = node
                    .as_obj()
                    .and_then(|o| o.get("pwdrequired"))
                    .and_then(|v| v.as_str())
                    .is_some_and(|v| v != "0");
                if !private {
                    names.push(name.to_string());
                }
            }
        }

        if !names.iter().any(|n| n == "public") {
            names.push("public".to_string());
        }

        names.sort();
        names.dedup();
        Ok(names)
    }

    pub async fn get_available_platforms(
        &mut self,
        appid: u32,
    ) -> Result<(Vec<DepotPlatform>, Vec<u8>)> {
        let buffer = self
            .fetch_appinfo_buffer(appid, "failed requesting appinfo product info")
            .await?;
        let appinfo_vdf_text = String::from_utf8_lossy(&buffer);

        let mut has_linux = false;
        let mut has_windows = false;

        let vdf_res = steam_vdf_parser::parse_binary(&buffer)
            .or_else(|_| steam_vdf_parser::parse_text(&appinfo_vdf_text).map(|v| v.into_owned()));

        if let Ok(vdf) = vdf_res {
            let root_obj = vdf.as_obj().unwrap();
            let depots_val = locate_depots(root_obj, vdf.key(), appid);

            if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
                for value in depots.values() {
                    let oslist = value
                        .get_obj(&["config"])
                        .and_then(|c| c.get("oslist"))
                        .and_then(|o| o.as_str());

                    if let Some(os) = oslist {
                        let os = os.to_lowercase();
                        if os.contains("linux") {
                            has_linux = true;
                        }
                        if os.contains("windows") {
                            has_windows = true;
                        }
                    }
                }
            }
        } else {
            tracing::warn!("get_available_platforms: VDF parse failed for {appid}, using fallback discovery");
            return Ok((vec![DepotPlatform::Windows, DepotPlatform::Linux], buffer));
        }

        let mut platforms = Vec::new();
        if has_windows {
            platforms.push(DepotPlatform::Windows);
        }
        if has_linux {
            platforms.push(DepotPlatform::Linux);
        }

        if platforms.is_empty() {
            platforms.push(DepotPlatform::Windows);
        }

        Ok((platforms, buffer))
    }

    /// Install `appid`'s depots for `platform`.
    ///
    /// `manifest_overrides` (depot → manifest) pins specific depots to an explicit
    /// (typically older) manifest id instead of the branch's current one — the
    /// engine behind `aurelia downgrade`. Overridden depots are always included even
    /// if the platform/language filter would otherwise drop them; depots without an
    /// override keep their current manifest. When any override is present the written
    /// appmanifest sets `AutoUpdateBehavior "1"` so the official client won't eagerly
    /// re-patch the pinned build.
    ///
    /// `branch`, when given, is used to resolve the build id recorded in the
    /// appmanifest (via PICS); `None` keeps the appinfo-derived (public) build id.
    #[allow(clippy::too_many_arguments)]
    pub async fn install_game(
        &self,
        appid: u32,
        platform: DepotPlatform,
        cached_vdf: Option<Vec<u8>>,
        filter_depots: Option<Vec<u64>>,
        library_override: Option<String>,
        manifest_overrides: Option<std::collections::HashMap<u32, u64>>,
        branch: Option<String>,
        shared_state: Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        let connection = self
            .connection
            .as_ref()
            .cloned()
            .context("steam connection not initialized")?;

        let cfg = load_launcher_config().await?;
        // Install into the caller-chosen library (a drive/location picked in the
        // UI) when given, else the configured default. DLCs ignore this and
        // follow their base game's library (handled below).
        let library_root = library_override.unwrap_or_else(|| cfg.steam_library_path.clone());
        let (game_name, pics_installdir) = self.resolve_install_game_info(appid).await;
        let installdir = pics_installdir.unwrap_or_else(|| sanitize_install_dir(&game_name));

        // If this app is a DLC, its content must land in the base game's install
        // directory and be registered in the base game's appmanifest (so the game
        // sees the DLC as installed/enabled) rather than getting its own manifest.
        let dlc_parent = self.resolve_dlc_parent(appid).await;
        let dlc_appid = dlc_parent.map(|_| appid);

        let (install_dir, manifest_path) = if let Some(base_appid) = dlc_parent {
            let base_manifest = self.appmanifest_path(base_appid).await?;
            if !base_manifest.exists() {
                bail!(
                    "cannot install DLC {appid}: its base game (app {base_appid}) is not installed — install it first"
                );
            }
            let base_raw = std::fs::read_to_string(&base_manifest)
                .with_context(|| format!("failed reading {}", base_manifest.display()))?;
            let base_installdir = parse_installdir_from_acf(&base_raw).ok_or_else(|| {
                anyhow!("could not determine base game install dir for app {base_appid}")
            })?;
            let steamapps = base_manifest
                .parent()
                .ok_or_else(|| anyhow!("invalid base manifest path for app {base_appid}"))?;
            let dir = steamapps.join("common").join(&base_installdir);
            tracing::info!(
                "DLC {appid} -> installing into base game {base_appid} at {}",
                dir.display()
            );
            (dir, base_manifest)
        } else {
            let dir = Path::new(&library_root)
                .join("steamapps")
                .join("common")
                .join(&installdir);
            let mp = Path::new(&library_root)
                .join("steamapps")
                .join(format!("appmanifest_{appid}.acf"));
            (dir, mp)
        };

        std::fs::create_dir_all(&install_dir)
            .with_context(|| format!("failed creating {}", install_dir.display()))?;

        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let client_clone = self.clone();
        let shared_state_clone = shared_state.clone();
        // A downgrade (any manifest override present) writes the appmanifest with
        // AutoUpdateBehavior "1" so the official client won't re-patch the pin.
        let pin_updates = manifest_overrides.is_some();

        tokio::task::spawn(async move {
            let _ = tx
                .send(DownloadProgress {
                    state: DownloadProgressState::Queued,
                    current_file: String::new(),
                    ..Default::default()
                })
                .await;

            let appinfo_vdf_bytes_owned;
            let appinfo_vdf_bytes = if let Some(cached) = cached_vdf {
                appinfo_vdf_bytes_owned = cached;
                &appinfo_vdf_bytes_owned
            } else {
                let mut request = CMsgClientPICSProductInfoRequest::new();
                request
                    .apps
                    .push(cmsg_client_picsproduct_info_request::AppInfo {
                        appid: Some(appid),
                        ..Default::default()
                    });

                let response: CMsgClientPICSProductInfoResponse = match connection.job(request).await
                {
                    Ok(res) => res,
                    Err(e) => {
                        let _ = tx
                            .send(DownloadProgress {
                                state: DownloadProgressState::Failed,
                                current_file: format!("failed requesting appinfo: {e}"),
                                ..Default::default()
                            })
                            .await;
                        return;
                    }
                };

                let app = response.apps.iter().find(|entry| entry.appid() == appid);
                let Some(app) = app else {
                    let _ = tx
                        .send(DownloadProgress {
                            state: DownloadProgressState::Failed,
                            current_file: "missing appinfo payload".to_string(),
                            ..Default::default()
                        })
                        .await;
                    return;
                };
                appinfo_vdf_bytes_owned = app.buffer().to_vec();
                &appinfo_vdf_bytes_owned
            };

            let mut selections = Vec::new();
            // Build id of the installed content (from PICS), recorded in the appmanifest
            // so the Steam launcher sees the install as current and doesn't re-download.
            let mut build_id: Option<String> = None;
            // Sum of all selected depots' max (uncompressed) sizes — the whole-app total
            // used to report overall download progress across depots.
            let mut grand_total_bytes: u64 = 0;

            let mut has_windows = false;
            if let Ok(map) = parse_pics_product_info(appinfo_vdf_bytes) {
                // To keep filtering, we re-parse or re-use the find_vdf logic.
                // We'll re-parse here to stay strictly compliant with Task 2's request to call parse_pics_product_info.
                if let Ok(vdf) = find_vdf_in_pics(appinfo_vdf_bytes) {
                    let root_obj = vdf.as_obj().unwrap();
                    let depots_val = if vdf.key() == "appinfo" || vdf.key() == appid.to_string() {
                        root_obj.get("depots")
                    } else {
                        root_obj.get("depots").or_else(|| {
                            root_obj
                                .get("appinfo")
                                .and_then(|v| v.as_obj())
                                .and_then(|o| o.get("depots"))
                        })
                    };

                    // depots -> branches -> public -> buildid
                    build_id = depots_val
                        .and_then(|d| d.get_obj(&["branches", "public"]))
                        .and_then(|b| b.get("buildid"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
                        for (key, value) in depots.iter() {
                            if let Ok(d_id) = key.parse::<u32>() {
                                let oslist = value
                                    .get_obj(&["config"])
                                    .and_then(|c| c.get("oslist"))
                                    .and_then(|o| o.as_str());

                                if oslist.is_some_and(|os| os.to_lowercase().contains("windows")) {
                                    has_windows = true;
                                }

                                // A manifest override for this depot takes precedence over
                                // its current manifest AND over the platform/language/filter
                                // gating — the user named this depot explicitly for a
                                // downgrade, so it is always included.
                                let override_mid = manifest_overrides
                                    .as_ref()
                                    .and_then(|m| m.get(&d_id).copied());

                                let manifest_id = if let Some(mid) = override_mid {
                                    Some(mid)
                                } else {
                                    let mut match_os = should_keep_depot(oslist, platform);

                                    if match_os {
                                        // 1. LANGUAGE CHECK
                                        let lang = value
                                            .get_obj(&["config"])
                                            .and_then(|c| c.get("language"))
                                            .and_then(|l| l.as_str());
                                        if let Some(lang) = lang {
                                            if lang != "english" && !lang.is_empty() {
                                                match_os = false;
                                            }
                                        }
                                    }

                                    let depot_id_u64 = d_id as u64;
                                    let is_allowed = filter_depots
                                        .as_ref()
                                        .is_none_or(|list| list.contains(&depot_id_u64));

                                    if match_os && is_allowed {
                                        map.get(&depot_id_u64).copied()
                                    } else {
                                        None
                                    }
                                };

                                if let Some(manifest_id) = manifest_id {
                                    // Uncompressed size for this depot. Prefer the
                                    // per-manifest size (present even when the
                                    // depot-level "maxsize" is absent/zero). For an
                                    // overridden (older) manifest this is only the
                                    // current build's size — a progress-bar estimate.
                                    grand_total_bytes += value
                                        .get_obj(&["manifests", "public"])
                                        .and_then(|m| m.get("size"))
                                        .and_then(|v| v.as_str())
                                        .and_then(|s| s.parse::<u64>().ok())
                                        .or_else(|| {
                                            value
                                                .get("maxsize")
                                                .and_then(|v| v.as_str())
                                                .and_then(|s| s.parse::<u64>().ok())
                                        })
                                        .unwrap_or(0);
                                    selections.push(ManifestSelection {
                                        app_id: appid,
                                        depot_id: d_id,
                                        manifest_id,
                                        // The download loop below consumes only
                                        // depot_id / manifest_id; the appinfo VDF is
                                        // never read from these selections, so avoid
                                        // cloning the (potentially multi-MB) VDF text
                                        // once per depot. (Mirrors launch.rs, which
                                        // also leaves this empty.)
                                        appinfo_vdf: String::new(),
                                    });
                                }
                            }
                        }
                    }
                }
            } else {
                tracing::error!("VDF parse failed for app {appid}");
            }

            if selections.is_empty() {
                let msg = if has_windows && matches!(platform, DepotPlatform::Linux) {
                    "No native Linux depots found. This game may only support Windows (Proton)."
                } else {
                    "No matching depots found for the selected platform."
                };

                let _ = tx
                    .send(DownloadProgress {
                        state: DownloadProgressState::Failed,
                        current_file: msg.to_string(),
                        ..Default::default()
                    })
                    .await;
                return;
            }

            // For a downgrade against a named branch, record that branch's build id in
            // the appmanifest instead of the appinfo-derived (public) one.
            if let Some(branch_name) = branch.as_deref() {
                if let Some(bid) =
                    SteamClient::remote_buildid_static(&connection, appid, branch_name).await
                {
                    build_id = Some(bid);
                }
            }

            let _ = tx
                .send(DownloadProgress {
                    state: DownloadProgressState::Downloading,
                    total_bytes: grand_total_bytes,
                    current_file: format!("starting download of {} depots", selections.len()),
                    ..Default::default()
                })
                .await;

            // Update shared state for the start of the download
            if let Ok(mut state) = shared_state_clone.write() {
                state.is_downloading = true;
                state.is_paused = false;
                state.app_id = appid;
                state.app_name = game_name.clone();
                state.downloaded_bytes = 0;
                // Whole-app total (all selected depots), so progress is reported against
                // the full install size rather than just the current depot.
                state.total_bytes = grand_total_bytes;
                state.status_text = format!("Initializing download for {}...", game_name);
            }

            // Register the install start with Steam: write an "update required"
            // appmanifest up front so the launcher sees the app as installing rather
            // than missing. (Skipped for DLC, whose content lives in the base game's
            // manifest — overwriting that here would mark the base game for re-download.)
            if dlc_appid.is_none() {
                if let Err(e) = SteamClient::write_appmanifest(
                    &manifest_path,
                    appid,
                    &game_name,
                    &installdir,
                    Vec::new(),
                    build_id.as_deref(),
                    false,
                    pin_updates,
                ) {
                    tracing::warn!("failed writing initial appmanifest for app {appid}: {e}");
                } else {
                    tracing::info!(
                        "Registered install start with Steam for app {appid} (buildid {})",
                        build_id.as_deref().unwrap_or("0")
                    );
                }
            }

            // Periodically forward the live byte counters over the channel. The
            // download callbacks only mutate the shared state; this reporter is what
            // turns that into the progress the CLI renders.
            let progress_tx = tx.clone();
            let progress_state = shared_state_clone.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_millis(250));
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
                    // Stop if the receiver is gone (terminal message already consumed).
                    if progress_tx
                        .send(DownloadProgress {
                            state: DownloadProgressState::Downloading,
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

            // 2. Fetch Content Servers via Service (once for all depots, so we hit the
            //    network only once regardless of how many depots are selected).
            tracing::info!("Fetching Content Servers for AppID: {}...", appid);
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

            // 3. Download Loop — delegates per-depot work to download_depot_to_with_hosts
            //    so the same CDN flow can be reused by Workshop downloads.
            let mut success = true;
            let mut successful_depots = Vec::new();
            for selection in selections {
                tracing::info!(
                    "Starting download for Depot {} (GID: {})...",
                    selection.depot_id,
                    selection.manifest_id
                );
                if let Ok(mut state) = shared_state_clone.write() {
                    state.status_text = format!("Downloading depot {}", selection.depot_id);
                    // Reset the current-depot counters so per-depot progress restarts.
                    state.depot_id = selection.depot_id;
                    state.depot_downloaded_bytes = 0;
                    state.depot_total_bytes = 0;
                }

                // A downgrade names this depot's manifest explicitly, so a request
                // code that comes back 401/empty (typical for very old manifests) is a
                // hard, clearly-reported failure rather than a silent skip.
                let is_override = manifest_overrides
                    .as_ref()
                    .is_some_and(|m| m.contains_key(&selection.depot_id));
                if is_override {
                    let ok = matches!(
                        client_clone
                            .get_manifest_request_code(
                                appid,
                                selection.depot_id,
                                selection.manifest_id,
                            )
                            .await,
                        Ok(code) if code != 0
                    );
                    if !ok {
                        let _ = tx
                            .send(DownloadProgress {
                                state: DownloadProgressState::Failed,
                                current_file: format!(
                                    "Steam declined a request code for manifest {} on depot {} — it may be too old, or require owning the game with a non-anonymous login",
                                    selection.manifest_id, selection.depot_id
                                ),
                                ..Default::default()
                            })
                            .await;
                        success = false;
                        break;
                    }
                }

                // Fetch the depot decryption key here so that a missing key
                // (not owned) silently skips this depot and continues to the
                // next — preserving the original install_game behavior. For an
                // overridden (downgrade) depot a missing key is instead a hard
                // error, since the user asked for that specific depot.
                let key = match client_clone.get_depot_key(appid, selection.depot_id).await {
                    Ok(k) => k,
                    Err(e) => {
                        if is_override {
                            let _ = tx
                                .send(DownloadProgress {
                                    state: DownloadProgressState::Failed,
                                    current_file: format!(
                                        "No depot key for depot {} — downgrade requires owning the game with a non-anonymous login: {}",
                                        selection.depot_id, e
                                    ),
                                    ..Default::default()
                                })
                                .await;
                            success = false;
                            break;
                        }
                        tracing::warn!(
                            "Skipping Depot {} (No Key/Not Owned): {}",
                            selection.depot_id,
                            e
                        );
                        continue;
                    }
                };
                // A valid depot key is exactly 32 bytes; a short/all-zero key would
                // decrypt chunks to garbage (the chunk path then fails the zip parse
                // with "Could not find EOCD").
                tracing::debug!(
                    "Depot {} key: {} bytes, all_zero={}",
                    selection.depot_id,
                    key.len(),
                    key.iter().all(|&b| b == 0)
                );

                match client_clone
                    .download_depot_to_with_hosts(
                        appid,
                        selection.depot_id,
                        selection.manifest_id,
                        &install_dir,
                        shared_state_clone.clone(),
                        &hosts,
                        grand_total_bytes,
                        &connection,
                        key,
                    )
                    .await
                {
                    Ok(depot_size) => {
                        successful_depots.push((
                            selection.depot_id,
                            selection.manifest_id,
                            depot_size,
                        ));
                    }
                    Err(e) => {
                        let aborted = shared_state_clone
                            .read()
                            .map(|s| {
                                s.abort_signal
                                    .load(std::sync::atomic::Ordering::Relaxed)
                            })
                            .unwrap_or(false);

                        if aborted {
                            success = false;
                            break;
                        }

                        let _ = tx
                            .send(DownloadProgress {
                                state: DownloadProgressState::Failed,
                                current_file: format!(
                                    "Failed to download depot {}: {}",
                                    selection.depot_id, e
                                ),
                                ..Default::default()
                            })
                            .await;
                        success = false;
                        break;
                    }
                }
            }

            if success {
                if let Ok(mut state) = shared_state_clone.write() {
                    state.is_downloading = false;
                    state.status_text = "Download complete".to_string();
                }

                let manifest_result = if let Some(dlc) = dlc_appid {
                    // Register the DLC's depots into the base game's manifest (enable it).
                    SteamClient::enable_dlc_in_appmanifest(&manifest_path, dlc, &successful_depots)
                } else {
                    SteamClient::write_appmanifest(
                        &manifest_path,
                        appid,
                        &game_name,
                        &installdir,
                        successful_depots,
                        build_id.as_deref(),
                        true,
                        pin_updates,
                    )
                };
                if let Err(err) = manifest_result {
                    tracing::warn!("failed updating appmanifest for {}: {}", appid, err);
                } else if dlc_appid.is_none() {
                    tracing::info!(
                        "Wrote appmanifest for app {appid}: fully installed, buildid {}",
                        build_id.as_deref().unwrap_or("0")
                    );
                }
                let _ = tx
                    .send(DownloadProgress {
                        state: DownloadProgressState::Completed,
                        bytes_downloaded: 1,
                        total_bytes: 1,
                        current_file: "completed".to_string(),
                        ..Default::default()
                    })
                    .await;
            } else {
                if let Ok(mut state) = shared_state_clone.write() {
                    state.is_downloading = false;
                    state.status_text = "Download failed".to_string();
                }
            }
        });

        Ok(rx)
    }

    /// Download a single depot's content (one manifest) into `dest`, reusing the
    /// content-server / manifest-request-code / depot-key / CDN flow. Tries each CDN
    /// host until one succeeds. Updates `shared_state` byte counters as it goes.
    /// Returns the depot's downloaded (uncompressed) size on success.
    ///
    /// This public entry-point fetches content servers internally (one network round-trip).
    /// It delegates to [`download_depot_to_with_hosts`] which accepts pre-fetched hosts
    /// — use that internal variant when you want a single host-fetch across many depots
    /// (as `install_game` does).
    pub(crate) async fn download_depot_to(
        &self,
        app_id: u32,
        depot_id: u32,
        manifest_id: u64,
        dest: &std::path::Path,
        shared_state: Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
    ) -> anyhow::Result<u64> {
        let connection = self
            .connection
            .as_ref()
            .cloned()
            .context("steam connection not initialized")?;

        let hosts = self.get_content_servers(connection.cell_id()).await?;
        let key = self.get_depot_key(app_id, depot_id).await
            .with_context(|| format!("failed to get depot key for depot {depot_id}"))?;

        // grand_total_bytes = 0: this standalone call has no whole-app total, so
        // on_manifest will accumulate per-depot totals into state.total_bytes.
        self.download_depot_to_with_hosts(
            app_id,
            depot_id,
            manifest_id,
            dest,
            shared_state,
            &hosts,
            0,
            &connection,
            key,
        )
        .await
    }

    /// Internal variant of [`download_depot_to`] that accepts pre-fetched CDN hosts,
    /// the whole-app grand total (for accurate overall progress), and a pre-fetched
    /// depot decryption key. Used by `install_game` so that (a) content servers are
    /// fetched only once for all depots and (b) a missing key can be handled as a
    /// silent skip at the call site rather than an error.
    ///
    /// `grand_total_bytes`: the sum of all selected depots' sizes (0 when unknown),
    /// used to decide whether to add this depot's size to `state.total_bytes`.
    pub(crate) async fn download_depot_to_with_hosts(
        &self,
        app_id: u32,
        depot_id: u32,
        manifest_id: u64,
        dest: &std::path::Path,
        shared_state: Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
        hosts: &[String],
        grand_total_bytes: u64,
        connection: &Connection,
        key: Vec<u8>,
    ) -> anyhow::Result<u64> {
        // A valid depot key is exactly 32 bytes; a short/all-zero key would
        // decrypt chunks to garbage (the chunk path then fails the zip parse
        // with "Could not find EOCD").
        tracing::debug!(
            "Depot {} key: {} bytes, all_zero={}",
            depot_id,
            key.len(),
            key.iter().all(|&b| b == 0)
        );

        let manifest_code = match self
            .get_manifest_request_code(app_id, depot_id, manifest_id)
            .await
        {
            Ok(code) => Some(code),
            Err(e) => {
                tracing::warn!(
                    "Failed to get manifest request code for depot {}: {}",
                    depot_id,
                    e
                );
                None
            }
        };

        let depot_size = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut depot_success = false;

        for host in hosts {
            let token = match self.get_cdn_auth_token(app_id, depot_id, host).await {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::warn!("Failed to get auth token for host {}: {}", host, e);
                    None
                }
            };

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

            let cdn_client =
                steam_cdn::CDNClient::with_server(Arc::new(connection.clone()), cdn_server);

            let state_for_closure = shared_state.clone();
            let on_progress = Arc::new(move |bytes: u64| {
                if let Ok(mut state) = state_for_closure.write() {
                    // Overall (whole app) and current-depot counters.
                    state.downloaded_bytes += bytes;
                    state.depot_downloaded_bytes += bytes;
                }
            });

            let state_for_manifest = shared_state.clone();
            let size_clone = depot_size.clone();
            let grand_total_fallback = grand_total_bytes;
            let on_manifest = Arc::new(move |total_bytes: u64| {
                size_clone.store(total_bytes, std::sync::atomic::Ordering::SeqCst);
                if let Ok(mut state) = state_for_manifest.write() {
                    // The manifest gives this depot's exact uncompressed size.
                    state.depot_total_bytes = total_bytes;
                    // If PICS carried no maxsize for the whole app, fall back to
                    // accumulating per-depot totals so overall progress still has a
                    // denominator.
                    if grand_total_fallback == 0 {
                        state.total_bytes += total_bytes;
                    }
                }
            });

            let abort_signal = shared_state
                .read()
                .ok()
                .map(|s| s.abort_signal.clone());

            match cdn_client
                .download_depot(
                    app_id,
                    depot_id,
                    manifest_id,
                    &key,
                    dest,
                    manifest_code,
                    false, // verify_mode: false
                    abort_signal,
                    Some(on_progress),
                    Some(on_manifest.clone()),
                )
                .await
            {
                Ok(_) => {
                    let aborted = shared_state
                        .read()
                        .map(|s| {
                            s.abort_signal
                                .load(std::sync::atomic::Ordering::Relaxed)
                        })
                        .unwrap_or(false);
                    if aborted {
                        anyhow::bail!("download aborted");
                    }

                    tracing::info!("Depot {} download complete from {}!", depot_id, host);
                    depot_success = true;
                    break;
                }
                Err(e) => {
                    tracing::error!("CDN Error from {}: {}", host, e);
                }
            }
        }

        if depot_success {
            Ok(depot_size.load(std::sync::atomic::Ordering::SeqCst))
        } else {
            anyhow::bail!(
                "Failed to download depot {} from all available servers",
                depot_id
            );
        }
    }
}
