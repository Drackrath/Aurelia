//! `SteamClient` methods: content servers, manifests, CDN, depots, sizes, launch options, achievements.
//!
//! Split out of `steam_client.rs` for readability; the struct, shared imports
//! and free helpers live in the parent module (in scope via `use super::*`).
use super::*;

impl SteamClient {
    /// Request PICS appinfo for a single app and return that app's raw VDF buffer.
    /// `job_context` is attached to the network call so each caller's error message
    /// is preserved. Shared by the depot/size/launch-option readers below.
    async fn request_app_pics_buffer(
        &self,
        app_id: u32,
        job_context: &'static str,
    ) -> Result<Vec<u8>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut request = CMsgClientPICSProductInfoRequest::new();
        request
            .apps
            .push(cmsg_client_picsproduct_info_request::AppInfo {
                appid: Some(app_id),
                ..Default::default()
            });

        let response: CMsgClientPICSProductInfoResponse =
            connection.job(request).await.context(job_context)?;

        let app = response
            .apps
            .iter()
            .find(|entry| entry.appid() == app_id)
            .ok_or_else(|| anyhow!("missing appinfo payload for app {app_id}"))?;

        Ok(app.buffer().to_vec())
    }

    /// Resolve the `depots` object from a parsed PICS VDF, descending past the
    /// numeric/`appinfo` wrapper when the depots aren't already at the root.
    fn locate_depots_value<'a>(
        vdf: &'a steam_vdf_parser::Vdf<'static>,
        app_id: u32,
    ) -> Option<&'a steam_vdf_parser::Value<'static>> {
        let root_obj = vdf.as_obj()?;
        if vdf.key() == "appinfo" || vdf.key() == app_id.to_string() {
            root_obj.get("depots")
        } else {
            root_obj.get("depots").or_else(|| {
                root_obj
                    .get("appinfo")
                    .and_then(|v| v.as_obj())
                    .and_then(|o| o.get("depots"))
            })
        }
    }

    pub async fn get_content_servers(&self, cell_id: u32) -> Result<Vec<String>> {
        let connection = self.connection.as_ref().ok_or_else(|| anyhow!("No connection"))?;
        let mut request = CContentServerDirectory_GetServersForSteamPipe_Request::new();
        request.set_cell_id(cell_id);
        request.set_max_servers(20);

        let response: CContentServerDirectory_GetServersForSteamPipe_Response = connection
            .service_method(request)
            .await
            .context("failed calling ContentServerDirectory.GetServersForSteamPipe")?;

        let hosts: Vec<String> = response
            .servers
            .iter()
            .filter(|server| matches!(server.type_(), "SteamCache" | "CDN"))
            .map(|server| server.host().to_string())
            .collect();

        if hosts.is_empty() {
            tracing::error!("ContentServerDirectory returned 0 valid CDN servers");
        }

        Ok(hosts)
    }

    pub async fn get_manifest_request_code(
        &self,
        app_id: u32,
        depot_id: u32,
        manifest_id: u64,
    ) -> Result<u64> {
        let connection = self.connection.as_ref().ok_or_else(|| anyhow!("No connection"))?;
        let mut request = CContentServerDirectory_GetManifestRequestCode_Request::new();
        request.set_app_id(app_id);
        request.set_depot_id(depot_id);
        request.set_manifest_id(manifest_id);

        let response: CContentServerDirectory_GetManifestRequestCode_Response = connection
            .service_method(request)
            .await
            .context("failed calling ContentServerDirectory.GetManifestRequestCode")?;

        let code = response.manifest_request_code();
        // A 0 code means the service-method response came back empty/default — worth
        // surfacing because the subsequent CDN manifest fetch will then fail.
        if code == 0 {
            tracing::warn!(
                "GetManifestRequestCode returned 0 (empty response) for app {app_id} depot {depot_id} manifest {manifest_id}"
            );
        } else {
            tracing::debug!(
                "GetManifestRequestCode for app {app_id} depot {depot_id} manifest {manifest_id} = {code}"
            );
        }
        Ok(code)
    }

    pub async fn get_cdn_auth_token(
        &self,
        app_id: u32,
        depot_id: u32,
        host_name: &str,
    ) -> Result<String> {
        let connection = self.connection.as_ref().ok_or_else(|| anyhow!("No connection"))?;
        let mut request = CContentServerDirectory_GetCDNAuthToken_Request::new();
        request.set_app_id(app_id);
        request.set_depot_id(depot_id);
        request.set_host_name(host_name.to_string());

        let response: CContentServerDirectory_GetCDNAuthToken_Response = connection
            .service_method(request)
            .await
            .context("failed calling ContentServerDirectory.GetCDNAuthToken")?;

        if response.token().is_empty() {
            // An empty token with the expiration field still set is a normal Steam
            // response (many SteamPipe CDNs don't require a per-host token), so this is
            // only a debug-level note, not an anomaly.
            tracing::debug!(
                "GetCDNAuthToken returned an empty token for app {app_id} depot {depot_id} host {host_name} (has_expiration={})",
                response.has_expiration_time()
            );
            return Err(anyhow!("Empty Auth Token returned"));
        }

        tracing::debug!(
            "GetCDNAuthToken for app {app_id} depot {depot_id} host {host_name}: token len {}",
            response.token().len()
        );
        Ok(response.token().to_string())
    }

    pub async fn get_depot_list(&self, app_id: u32) -> Result<Vec<DepotInfo>> {
        let buffer = self
            .request_app_pics_buffer(app_id, "failed requesting appinfo product info for depot list")
            .await?;

        let mut out = Vec::new();
        if let Ok(vdf) = find_vdf_in_pics(&buffer) {
            vdf.as_obj().context("root is not an object")?;
            let depots_val = Self::locate_depots_value(&vdf, app_id);

            if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
                for (key, value) in depots.iter() {
                    if let (Ok(d_id), Some(obj)) = (key.parse::<u64>(), value.as_obj()) {
                        let name = obj
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("Depot {d_id}"));

                        let size = obj
                            .get("maxsize")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(0);

                        let mut config_parts = Vec::new();
                        if let Some(config) = obj.get("config").and_then(|v| v.as_obj()) {
                            if let Some(os) = config.get("oslist").and_then(|v| v.as_str()) {
                                config_parts.push(format!("os: {}", os));
                            }
                            if let Some(lang) = config.get("language").and_then(|v| v.as_str()) {
                                config_parts.push(format!("lang: {}", lang));
                            }
                        }

                        out.push(DepotInfo {
                            id: d_id,
                            name,
                            size,
                            file_count: 0, // Not easily available in PICS VDF without manifest
                            config: config_parts.join(", "),
                            is_owned: None,
                        });
                    }
                }
            }
        }

        out.sort_by_key(|d| d.id);
        Ok(out)
    }

    /// List, per depot, the *current* manifest id on every branch that PICS
    /// advertises (the version-discovery data behind `aurelia manifests`).
    ///
    /// Steam's protocol only exposes the current manifest per branch, so this
    /// never returns historical/older ids — those live on SteamDB. Reuses the same
    /// appinfo `depots -> <depot> -> manifests -> <branch> -> gid/size` walk the
    /// install pipeline reads.
    pub async fn list_depot_manifests(&self, app_id: u32) -> Result<Vec<DepotManifestInfo>> {
        let buffer = self
            .request_app_pics_buffer(app_id, "failed requesting appinfo product info for manifests")
            .await?;

        // Parse a gid value that may be encoded as a quoted string or a raw u64.
        fn parse_gid(v: &steam_vdf_parser::Value) -> Option<u64> {
            if let Some(s) = v.as_str() {
                return s.parse::<u64>().ok();
            }
            v.as_u64()
        }

        let mut out = Vec::new();
        if let Ok(vdf) = find_vdf_in_pics(&buffer) {
            let depots_val = Self::locate_depots_value(&vdf, app_id);
            if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
                for (key, value) in depots.iter() {
                    let Ok(depot_id) = key.parse::<u32>() else { continue };
                    let Some(obj) = value.as_obj() else { continue };
                    let depot_name = obj.get("name").and_then(|v| v.as_str()).map(str::to_string);

                    let Some(manifests) = obj.get("manifests").and_then(|v| v.as_obj()) else {
                        continue;
                    };
                    for (branch, entry) in manifests.iter() {
                        // A branch entry is either an object ({ gid, size, download })
                        // or, on older appinfo, a bare gid string.
                        let (manifest_id, size) = match entry.as_obj() {
                            Some(bo) => (
                                bo.get("gid").and_then(parse_gid),
                                bo.get("size")
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| s.parse::<u64>().ok())
                                    .unwrap_or(0),
                            ),
                            None => (parse_gid(entry), 0),
                        };
                        if let Some(manifest_id) = manifest_id {
                            out.push(DepotManifestInfo {
                                depot_id,
                                depot_name: depot_name.clone(),
                                branch: branch.to_string(),
                                manifest_id,
                                size,
                            });
                        }
                    }
                }
            }
        }

        out.sort_by(|a, b| a.depot_id.cmp(&b.depot_id).then(a.branch.cmp(&b.branch)));
        Ok(out)
    }

    /// Estimate the download and on-disk size of installing `app_id` on `platform`,
    /// without fetching any manifests. Reads each depot's `manifests.public.size`
    /// (disk) and `manifests.public.download` (compressed) from PICS appinfo and
    /// sums the depots that match the target platform — mirroring the install
    /// pipeline's [`should_keep_depot`] selection. DLC depots (`dlcappid`) are
    /// excluded, so this estimates the base game; DLC sizing isn't covered.
    pub async fn estimate_install_size(
        &self,
        app_id: u32,
        platform: DepotPlatform,
    ) -> Result<InstallSizeEstimate> {
        let buffer = self
            .request_app_pics_buffer(
                app_id,
                "failed requesting appinfo product info for size estimate",
            )
            .await?;

        let mut est = InstallSizeEstimate::default();
        let vdf = find_vdf_in_pics(&buffer).context("failed to parse product info VDF")?;
        vdf.as_obj().context("root is not an object")?;
        let depots_val = Self::locate_depots_value(&vdf, app_id);

        if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
            for (key, value) in depots.iter() {
                // Only numeric keys are depots (skip `branches`, `overflowstorage`, …).
                if key.parse::<u64>().is_err() {
                    continue;
                }
                let Some(obj) = value.as_obj() else { continue };

                // Exclude DLC content depots (estimate is for the base game).
                if obj.get("dlcappid").is_some() {
                    continue;
                }

                // Platform filter, matching the install pipeline.
                let oslist = obj
                    .get("config")
                    .and_then(|v| v.as_obj())
                    .and_then(|c| c.get("oslist"))
                    .and_then(|v| v.as_str());
                if !should_keep_depot(oslist, platform) {
                    continue;
                }

                let public = obj
                    .get("manifests")
                    .and_then(|v| v.as_obj())
                    .and_then(|m| m.get("public"))
                    .and_then(|v| v.as_obj());

                let disk = public
                    .and_then(|p| p.get("size"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
                    .or_else(|| {
                        obj.get("maxsize")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse::<u64>().ok())
                    })
                    .unwrap_or(0);
                // Steam's `download` is the compressed transfer size; fall back to the
                // uncompressed size when a depot doesn't advertise it.
                let download = public
                    .and_then(|p| p.get("download"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(disk);

                if disk > 0 || download > 0 {
                    est.disk_size += disk;
                    est.download_size += download;
                    est.depot_count += 1;
                }
            }
        }

        Ok(est)
    }

    /// List a game's launch options from its PICS `config/launch` table — the set
    /// of executables/arguments Steam can start the game with, plus their platform
    /// constraints. Read with the binary-safe VDF path (works for both binary and
    /// text PICS payloads). Entry `"0"` is sorted first (the default).
    pub async fn fetch_launch_options(&self, app_id: u32) -> Result<Vec<LaunchOptionInfo>> {
        let buffer = self
            .request_app_pics_buffer(
                app_id,
                "failed requesting appinfo product info for launch options",
            )
            .await?;

        let vdf = find_vdf_in_pics(&buffer).context("failed to parse product info VDF")?;
        let root_obj = vdf.as_obj().context("root is not an object")?;

        // `config` sits at the root or under the numeric/"appinfo" wrapper.
        let config = root_obj.get("config").and_then(|v| v.as_obj()).or_else(|| {
            root_obj
                .get("appinfo")
                .and_then(|v| v.as_obj())
                .and_then(|o| o.get("config"))
                .and_then(|v| v.as_obj())
        });

        let mut out = Vec::new();
        if let Some(launch) = config.and_then(|c| c.get("launch")).and_then(|v| v.as_obj()) {
            for (id, entry) in launch.iter() {
                let Some(e) = entry.as_obj() else { continue };
                let field = |k: &str| e.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
                let cfg = e.get("config").and_then(|v| v.as_obj());
                let cfg_field = |k: &str| {
                    cfg.and_then(|c| c.get(k))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                };

                out.push(LaunchOptionInfo {
                    id: id.to_string(),
                    description: field("description"),
                    executable: field("executable"),
                    arguments: field("arguments"),
                    working_dir: field("workingdir"),
                    oslist: cfg_field("oslist"),
                    osarch: cfg_field("osarch"),
                    launch_type: field("type"),
                });
            }
        }

        // Default entry ("0") first, then by id.
        out.sort_by(|a, b| match (a.id.as_str(), b.id.as_str()) {
            ("0", "0") => std::cmp::Ordering::Equal,
            ("0", _) => std::cmp::Ordering::Less,
            (_, "0") => std::cmp::Ordering::Greater,
            _ => a.id.cmp(&b.id),
        });
        Ok(out)
    }

    /// Fetch the logged-in user's achievements for a game, combining the game's
    /// achievement definitions + global rarity (`Player.GetGameAchievements`) with
    /// the user's per-achievement unlock state and time (`ClientGetUserStats`,
    /// whose binary-KV schema maps each achievement to its stat/bit). Achievements
    /// the user hasn't unlocked are returned with `unlocked = false`.
    pub async fn fetch_achievements(
        &self,
        appid: u32,
        language: &str,
    ) -> Result<Vec<GameAchievement>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        let steam_id = self
            .steam_id()
            .context("not logged in — achievements need an authenticated session")?;

        // 1. Definitions + global rarity (localized).
        let mut def_req = CPlayer_GetGameAchievements_Request::new();
        def_req.set_appid(appid);
        def_req.set_language(language.to_string());
        let def_resp: CPlayer_GetGameAchievements_Response = connection
            .service_method(def_req)
            .await
            .context("Player.GetGameAchievements failed")?;

        // 2. The user's unlock state. A user who never launched the game returns no
        //    blocks (everything stays locked) — not an error.
        let mut stats_req = CMsgClientGetUserStats::new();
        stats_req.set_game_id(u64::from(appid));
        stats_req.set_steam_id_for_user(steam_id);
        let stats_resp: CMsgClientGetUserStatsResponse = connection
            .job(stats_req)
            .await
            .context("ClientGetUserStats failed")?;

        // api-name -> (stat_id, bit), parsed from the binary-KV schema.
        let bit_index = parse_achievement_schema(stats_resp.schema());
        // Case-insensitive fallback (some games' definition vs schema names differ in case).
        let bit_index_ci: HashMap<String, (u32, u32)> = bit_index
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), *v))
            .collect();
        // stat_id -> unlock_time vector (indexed by bit).
        let mut unlock_by_stat: HashMap<u32, &Vec<u32>> = HashMap::new();
        for block in &stats_resp.achievement_blocks {
            unlock_by_stat.insert(block.achievement_id(), &block.unlock_time);
        }
        tracing::debug!(
            appid,
            eresult = stats_resp.eresult(),
            schema_bytes = stats_resp.schema().len(),
            schema_achievements = bit_index.len(),
            unlock_blocks = unlock_by_stat.len(),
            "achievements: parsed user-stats schema"
        );
        if !stats_resp.schema().is_empty() && bit_index.is_empty() {
            tracing::warn!(
                appid,
                schema_bytes = stats_resp.schema().len(),
                "achievement schema present but parsed 0 entries; unlock state unavailable"
            );
        }

        let mut out = Vec::new();
        for ach in &def_resp.achievements {
            let api = ach.internal_name().to_string();
            let mapped = bit_index
                .get(&api)
                .or_else(|| bit_index_ci.get(&api.to_ascii_lowercase()))
                .copied();
            let (unlocked, unlock_time) = match mapped {
                Some((stat_id, bit)) => {
                    let t = unlock_by_stat
                        .get(&stat_id)
                        .and_then(|times| times.get(bit as usize))
                        .copied()
                        .unwrap_or(0);
                    if t > 0 { (true, Some(t)) } else { (false, None) }
                }
                None => (false, None),
            };

            out.push(GameAchievement {
                name: ach.localized_name().to_string(),
                description: ach.localized_desc().to_string(),
                hidden: ach.hidden(),
                icon_unlocked: achievement_icon_url(appid, ach.icon()),
                icon_locked: achievement_icon_url(appid, ach.icon_gray()),
                global_percent: ach.player_percent_unlocked().parse::<f32>().unwrap_or(0.0),
                unlocked,
                unlock_time,
                api_name: api,
            });
        }
        Ok(out)
    }

    pub async fn get_depot_key(&self, app_id: u32, depot_id: u32) -> Result<Vec<u8>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        let mut request = CMsgClientGetDepotDecryptionKey::new();
        request.set_depot_id(depot_id);
        request.set_app_id(app_id);

        let response: CMsgClientGetDepotDecryptionKeyResponse = connection.job(request).await?;
        if response.eresult() != 1 {
            bail!(
                "failed to get depot key for depot {depot_id}: eresult {}",
                response.eresult()
            );
        }

        Ok(response.depot_encryption_key().to_vec())
    }

    pub async fn verify_depot_ownership(&self, app_id: u32, depot_ids: Vec<u64>) -> HashMap<u64, bool> {
        tracing::info!("Verifying ownership for {} depots...", depot_ids.len());
        let mut results = HashMap::new();

        let Some(connection) = self.connection.as_ref() else {
            results.extend(depot_ids.into_iter().map(|id| (id, false)));
            return results;
        };

        // 1. Ensure we have an App Ticket (Warm up session)
        let _ = self.get_app_ticket(app_id).await;

        for depot_id in depot_ids {
            let mut request = CMsgClientGetDepotDecryptionKey::new();
            request.set_depot_id(depot_id as u32);
            request.set_app_id(app_id);

            let response: std::result::Result<CMsgClientGetDepotDecryptionKeyResponse, _> =
                connection.job(request).await;
            // EResult::OK == 1
            let owned = matches!(response, Ok(r) if r.eresult() == 1);
            results.insert(depot_id, owned);
        }
        results
    }

    pub async fn fetch_depots(&self, appid: u32) -> Result<Vec<BrowserDepotInfo>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        depot_browser::fetch_depots(connection, appid).await
    }

    pub async fn fetch_manifest_files(
        &self,
        appid: u32,
        depot_id: u32,
        manifest_ref: &str,
    ) -> Result<Vec<ManifestFileEntry>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        depot_browser::fetch_manifest_files(connection, appid, depot_id, manifest_ref).await
    }

    pub fn download_single_file(
        &self,
        appid: u32,
        depot_id: u32,
        manifest_ref: &str,
        file_path: &str,
        output_dir: &Path,
    ) -> Result<()> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        depot_browser::download_single_file(
            connection,
            appid,
            depot_id,
            manifest_ref,
            file_path,
            output_dir,
        )
    }

}
