//! `SteamClient` methods: owned/family games, profile, app metadata, store info, product info.
//!
//! Split out of `steam_client.rs` for readability; the struct, shared imports
//! and free helpers live in the parent module (in scope via `use super::*`).
use super::*;

impl SteamClient {
    pub async fn fetch_owned_games(&mut self) -> Result<Vec<OwnedGame>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let request = CPlayer_GetOwnedGames_Request {
            steamid: Some(u64::from(connection.steam_id())),
            include_appinfo: Some(true),
            include_played_free_games: Some(true),
            ..Default::default()
        };

        tracing::debug!("Calling Player.GetOwnedGames ...");
        let response: CPlayer_GetOwnedGames_Response = connection
            .service_method(request)
            .await
            .context("failed calling Player.GetOwnedGames")?;
        tracing::debug!("Player.GetOwnedGames returned {} games", response.games.len());

        let owned: Vec<OwnedGame> = response
            .games
            .iter()
            .map(|game| OwnedGame {
                app_id: game.appid() as u32,
                name: if game.name().is_empty() {
                    format!("App {}", game.appid())
                } else {
                    game.name().to_string()
                },
                playtime_forever_minutes: game.playtime_forever() as u32,
                local_manifest_ids: HashMap::new(),
                update_available: false,
            })
            .collect();

        save_library_cache(&owned).await.ok();
        Ok(owned)
    }

    /// Fetch games available to this account through Steam Family Sharing that the
    /// account does **not** itself own. Returns an empty list if the account is not
    /// part of a family group. These may or may not be installed locally.
    pub async fn fetch_family_shared_apps(&self) -> Result<Vec<SharedApp>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        let my_steamid = u64::from(connection.steam_id());

        // 1. Resolve the family group this account belongs to.
        let mut group_req = CFamilyGroups_GetFamilyGroupForUser_Request::new();
        group_req.set_steamid(my_steamid);
        group_req.set_include_family_group_response(true);

        tracing::debug!("Calling FamilyGroups.GetFamilyGroupForUser ...");
        let group_resp: CFamilyGroups_GetFamilyGroupForUser_Response = connection
            .service_method(group_req)
            .await
            .context("failed calling FamilyGroups.GetFamilyGroupForUser")?;

        let family_groupid = group_resp.family_groupid();
        if family_groupid == 0 {
            // Account is not in a family group; nothing is shared with it.
            return Ok(Vec::new());
        }

        // 2. List apps shared with us by other family members (exclude our own).
        let mut apps_req = CFamilyGroups_GetSharedLibraryApps_Request::new();
        apps_req.set_family_groupid(family_groupid);
        apps_req.set_steamid(my_steamid);
        apps_req.set_include_own(false);
        apps_req.set_include_excluded(false);
        apps_req.set_include_non_games(false);
        apps_req.set_max_apps(10_000);
        apps_req.set_language("english".to_string());

        let apps_resp: CFamilyGroups_GetSharedLibraryApps_Response = connection
            .service_method(apps_req)
            .await
            .context("failed calling FamilyGroups.GetSharedLibraryApps")?;

        let shared = apps_resp
            .apps
            .iter()
            .map(|app| {
                let app_id = app.appid();
                SharedApp {
                    app_id,
                    name: if app.name().is_empty() {
                        format!("App {app_id}")
                    } else {
                        app.name().to_string()
                    },
                    owner_steamid: app.owner_steamids.first().copied(),
                }
            })
            .collect();
        Ok(shared)
    }

    pub async fn refresh_owned_games(&mut self, _session: &SessionState) -> Result<Vec<OwnedGame>> {
        self.fetch_owned_games().await
    }

    pub async fn load_cached_owned_games(&self) -> Result<Vec<OwnedGame>> {
        load_library_cache().await
    }

    pub async fn check_for_updates(&self, games: &mut [LibraryGame]) -> Result<()> {
        for game in games.iter_mut() {
            game.update_available = false;
            game.local_manifest_ids.clear();

            if !game.is_installed {
                continue;
            }

            let (local, branch) = self.local_manifest_info(game)?;
            game.local_manifest_ids = local.clone();
            game.active_branch = branch;

            if self.is_offline() || self.connection.is_none() {
                continue;
            }

            let remote = self
                .remote_manifest_ids(game.app_id, &game.active_branch)
                .await
                .unwrap_or_default();
            if remote.is_empty() {
                continue;
            }

            game.update_available = remote.iter().any(|(depot, remote_manifest)| {
                local.get(depot).copied().unwrap_or_default() != *remote_manifest
            });
        }

        Ok(())
    }

    pub(crate) fn local_manifest_info(&self, game: &LibraryGame) -> Result<(HashMap<u64, u64>, String)> {
        let install_path = match &game.install_path {
            Some(path) => PathBuf::from(path),
            None => return Ok((HashMap::new(), "public".to_string())),
        };

        let steamapps = match install_path.parent().and_then(|p| p.parent()) {
            Some(path) => path.to_path_buf(),
            None => return Ok((HashMap::new(), "public".to_string())),
        };

        let manifest_path = steamapps.join(format!("appmanifest_{}.acf", game.app_id));
        if !manifest_path.exists() {
            return Ok((HashMap::new(), "public".to_string()));
        }

        let raw = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed reading {}", manifest_path.display()))?;
        let manifests = parse_installed_depots_from_acf(&raw);
        let branch = parse_active_branch_from_acf(&raw);
        Ok((manifests, branch))
    }

    pub(crate) async fn remote_manifest_ids(&self, appid: u32, branch: &str) -> Result<HashMap<u64, u64>> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        SteamClient::remote_manifest_ids_static(connection, appid, branch).await
    }

    pub async fn get_user_profile(&self, current_library_len: usize) -> Result<UserProfile> {
        let persisted = load_session().await.unwrap_or_default();
        let account_name = persisted
            .account_name
            .unwrap_or_else(|| "Unknown User".to_string());

        if self.is_offline() {
            let cached_games = load_library_cache().await.unwrap_or_default();
            return Ok(UserProfile {
                steam_id: persisted.steam_id.unwrap_or_default(),
                account_name,
                game_count: cached_games.len(),
                is_online: false,
            });
        }

        let steam_id = self
            .connection
            .as_ref()
            .map(|connection| u64::from(connection.steam_id()))
            .or(persisted.steam_id)
            .unwrap_or_default();

        Ok(UserProfile {
            steam_id,
            account_name,
            game_count: current_library_len,
            is_online: true,
        })
    }

    /// Fetch one app's raw PICS product-info buffer (usually *binary* VDF).
    async fn fetch_pics_buffer(&self, appid: u32, request_context: &'static str) -> Result<Vec<u8>> {
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

        let response: CMsgClientPICSProductInfoResponse = connection
            .job(request)
            .await
            .context(request_context)?;

        let app = response
            .apps
            .iter()
            .find(|entry| entry.appid() == appid)
            .ok_or_else(|| anyhow!("missing appinfo payload for app {appid}"))?;
        Ok(app.buffer().to_vec())
    }

    pub async fn get_extended_app_info(&self, appid: u32) -> Result<ExtendedAppInfo> {
        let buffer = self
            .fetch_pics_buffer(appid, "failed requesting appinfo product info for extended metadata")
            .await?;

        // PICS product-info buffers are usually *binary* VDF (text only for some
        // apps), so parse via `find_vdf_in_pics` rather than the text-only
        // `parse_appinfo` — otherwise binary appinfo silently yields no DLC/depots.
        // Extract everything in a block so the borrowed VDF tree is dropped before
        // the later `.await`.
        let (name, dlcs, depots, launch_options) = {
            let vdf = find_vdf_in_pics(&buffer)
                .context("failed to parse product info VDF")?;
            let section = pics_app_section(vdf.value());

            let name = section.get_str(&["common", "name"]).map(str::to_string);

            // The canonical DLC list is `extended/listofdlc` — a comma-separated
            // string of app ids.
            let dlcs = dlc_ids_from_section(section);

            let depots = depots_from_section(section);
            let launch_options = launch_options_from_section(section);

            (name, dlcs, depots, launch_options)
        };

        let manifest_path = self.appmanifest_path(appid).await?;
        let active_branch = if manifest_path.exists() {
            let raw = std::fs::read_to_string(&manifest_path).unwrap_or_default();
            parse_active_branch_from_acf(&raw)
        } else {
            "public".to_string()
        };

        Ok(ExtendedAppInfo {
            name,
            dlcs,
            depots,
            launch_options,
            active_branch,
        })
    }

    /// Fetch the app's Steam Auto-Cloud `savefiles` rules from PICS appinfo. These
    /// describe where the game's saves live on disk so Cloud sync can discover and
    /// upload brand-new local saves (not just files already in the cloud). Returns
    /// an empty list for apps with no UFS config.
    pub async fn fetch_ufs_save_specs(&self, appid: u32) -> Result<Vec<UfsSaveSpec>> {
        let buffer = self
            .fetch_pics_buffer(appid, "failed requesting appinfo product info for UFS save specs")
            .await?;

        let vdf = find_vdf_in_pics(&buffer).context("failed to parse product info VDF")?;
        Ok(ufs_save_specs_from_section(pics_app_section(vdf.value())))
    }

    /// Fetch a single app's PICS appinfo and infer whether it appears to require
    /// an online connection to play. Steam exposes no explicit flag for this, so
    /// the answer is derived from the app's store categories — see
    /// [`category_online_required`]. Requires an active Steam connection.
    pub async fn fetch_online_required(&self, appid: u32) -> Result<bool> {
        let buffer = self
            .fetch_pics_buffer(appid, "failed requesting appinfo product info for online-required check")
            .await?;

        // PICS product-info buffers are usually *binary* VDF (text only for some
        // apps), so go through `find_vdf_in_pics` rather than the text-only
        // `parse_appinfo`. `pics_app_section` descends past the `appinfo`/numeric
        // wrapper so the category map can be read directly at `common/category`.
        let vdf = find_vdf_in_pics(&buffer).context("failed to parse product info VDF")?;
        let section = pics_app_section(vdf.value());

        let mut categories = HashMap::new();
        if let Some(cat_obj) = section.get_obj(&["common", "category"]) {
            for (key, value) in cat_obj.iter() {
                if let Some(v) = value.as_str() {
                    categories.insert(key.to_string(), v.to_string());
                }
            }
        }

        Ok(category_online_required(&categories))
    }

    /// Fetch human-facing store metadata for one or more apps via the
    /// `StoreBrowse.GetItems` service method (over the CM connection — no HTTPS
    /// storefront API). Returns one [`StoreAppInfo`] per app the store knows
    /// about; unknown/region-locked ids are simply omitted. Requires a connection.
    /// `language` is a Steam API language name (e.g. "english", "german").
    pub async fn fetch_store_apps(
        &self,
        app_ids: &[u32],
        language: &str,
    ) -> Result<Vec<StoreAppInfo>> {
        if app_ids.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut data = StoreBrowseItemDataRequest::new();
        data.set_include_basic_info(true);
        data.set_include_release(true);
        data.set_include_platforms(true);
        data.set_include_reviews(true);
        data.set_include_all_purchase_options(true);
        data.set_include_full_description(true);
        data.set_include_assets(true);

        let mut context = StoreBrowseContext::new();
        context.set_language(language.to_string());
        context.set_country_code("US".to_string());

        let mut request = CStoreBrowse_GetItems_Request::new();
        request.context = MessageField::some(context);
        request.data_request = MessageField::some(data);
        for &id in app_ids {
            let mut item_id = StoreItemID::new();
            item_id.set_appid(id);
            request.ids.push(item_id);
        }

        let response: CStoreBrowse_GetItems_Response = connection
            .service_method(request)
            .await
            .context("failed calling StoreBrowse.GetItems")?;

        Ok(response
            .store_items
            .iter()
            .filter(|item| item.appid() != 0)
            .map(store_item_to_app_info)
            .collect())
    }

    pub async fn get_product_info(&mut self, appid: u32) -> Result<Vec<LaunchInfo>> {
        let buffer = self
            .fetch_pics_buffer(appid, "failed requesting appinfo product info for launch metadata")
            .await?;

        let raw_vdf = String::from_utf8_lossy(&buffer);
        if raw_vdf.trim().is_empty() {
            bail!("empty appinfo payload returned for app {appid}")
        }

        parse_launch_info_from_vdf(appid, &raw_vdf)
            .context("failed to parse launch metadata from PICS appinfo")
    }

}

/// Depot `(id, name)` pairs from an app's PICS section. The `depots` object holds
/// numeric depot-id keys alongside non-numeric siblings (`branches`,
/// `baselanguages`, …); only the numeric ones are kept.
fn depots_from_section(section: &steam_vdf_parser::Value) -> Vec<(u32, String)> {
    let mut depots = Vec::new();
    if let Some(depots_obj) = section.get_obj(&["depots"]) {
        for (id_str, node) in depots_obj.iter() {
            let Ok(id) = id_str.parse::<u32>() else { continue };
            let name = node
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown Depot")
                .to_string();
            depots.push((id, name));
        }
    }
    depots
}

/// Raw launch options (`executable`, `arguments`) from an app's PICS
/// `config/launch` section, in declaration order.
fn launch_options_from_section(section: &steam_vdf_parser::Value) -> Vec<RawLaunchOption> {
    let mut launch_options = Vec::new();
    if let Some(launch_obj) = section.get_obj(&["config", "launch"]) {
        for entry in launch_obj.values() {
            launch_options.push(RawLaunchOption {
                executable: entry
                    .get("executable")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                arguments: entry
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            });
        }
    }
    launch_options
}
