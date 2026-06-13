//! `SteamClient` methods: appmanifest paths, install-dir resolution, manifest writing, DLC state.
//!
//! Split out of `steam_client.rs` for readability; the struct, shared imports
//! and free helpers live in the parent module (in scope via `use super::*`).
use super::*;

impl SteamClient {
    pub(crate) async fn appmanifest_path(&self, appid: u32) -> Result<PathBuf> {
        let file = format!("appmanifest_{appid}.acf");

        // Search every known Steam library (incl. other drives) for the manifest.
        for root in crate::library::all_library_roots().await {
            let candidate = root.join("steamapps").join(&file);
            if candidate.exists() {
                return Ok(candidate);
            }
        }

        // Fall back to the configured library root even if the manifest is absent,
        // preserving the previous behaviour for callers that tolerate a missing file.
        let cfg = load_launcher_config().await?;
        Ok(PathBuf::from(cfg.steam_library_path)
            .join("steamapps")
            .join(file))
    }

    pub(crate) async fn local_manifest_info_for_appid(&self, appid: u32) -> Result<(HashMap<u64, u64>, String)> {
        let manifest_path = self.appmanifest_path(appid).await?;
        if !manifest_path.exists() {
            return Ok((HashMap::new(), "public".to_string()));
        }
        let raw = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed reading {}", manifest_path.display()))?;
        let manifests = parse_installed_depots_from_acf(&raw);
        let branch = parse_active_branch_from_acf(&raw);
        Ok((manifests, branch))
    }

    pub(crate) async fn install_root_for_app(&self, appid: u32) -> Result<PathBuf> {
        let manifest_path = self.appmanifest_path(appid).await?;
        let steamapps = manifest_path
            .parent()
            .ok_or_else(|| anyhow!("invalid steamapps path for app {appid}"))?
            .to_path_buf();

        if manifest_path.exists() {
            let raw = std::fs::read_to_string(&manifest_path)
                .with_context(|| format!("failed reading {}", manifest_path.display()))?;
            if let Some(installdir) = parse_installdir_from_acf(&raw) {
                let p = steamapps.join("common").join(&installdir);
                if p.exists() {
                    return Ok(p);
                }

                // Fallback: search for app id markers if the specified installdir doesn't exist
                if let Some(fallback) = self.probe_install_dir_by_appid(&steamapps, appid) {
                    tracing::info!("Found fallback install dir for app {appid}: {:?}", fallback);
                    return Ok(fallback);
                }

                // Even if it doesn't exist, we return the path it *should* be at
                return Ok(p);
            }
        }

        // Final fallback if no manifest or installdir
        Ok(PathBuf::from(load_launcher_config().await?.steam_library_path)
            .join("steamapps")
            .join("common")
            .join(appid.to_string()))
    }

    pub(crate) fn probe_install_dir_by_appid(&self, steamapps: &Path, appid: u32) -> Option<PathBuf> {
        let common = steamapps.join("common");
        if !common.exists() {
            return None;
        }

        let appid_str = appid.to_string();

        let entries = std::fs::read_dir(common).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            // Check for steam_appid.txt
            if path.is_dir() {
                if let Ok(content) = std::fs::read_to_string(path.join("steam_appid.txt")) {
                    if content.trim() == appid_str {
                        return Some(path);
                    }
                }
            }
        }
        None
    }

    /// Request the PICS appinfo product-info for a single app and return its raw
    /// appinfo buffer. The buffer is either text or binary VDF (see
    /// [`find_vdf_in_pics`] / [`parse_appinfo`]). Shared by every PICS appinfo
    /// lookup in this module so the request/job/find-app boilerplate lives once.
    async fn pics_app_buffer(connection: &Connection, appid: u32) -> Result<Vec<u8>> {
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
            .context("failed requesting appinfo product info for update metadata")?;
        let app = response
            .apps
            .iter()
            .find(|entry| entry.appid() == appid)
            .ok_or_else(|| anyhow!("missing appinfo payload for app {appid}"))?;
        Ok(app.buffer().to_vec())
    }

    /// Locate the `depots` object inside a PICS appinfo VDF, accounting for both
    /// the unwrapped layout (root holds the sections directly) and the wrapped
    /// layout (sections nested under an `appinfo` key).
    fn pics_depots_value<'a>(
        vdf: &'a steam_vdf_parser::Vdf<'static>,
        appid: u32,
    ) -> Option<&'a steam_vdf_parser::Value<'static>> {
        let root_obj = vdf.as_obj()?;
        if vdf.key() == "appinfo" || vdf.key() == appid.to_string() {
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

    pub(crate) async fn remote_manifest_ids_static(
        connection: &Connection,
        appid: u32,
        branch: &str,
    ) -> Result<HashMap<u64, u64>> {
        let buffer = Self::pics_app_buffer(connection, appid).await?;

        let mut manifests = HashMap::new();
        if let Ok(vdf) = find_vdf_in_pics(&buffer) {
            let depots_val = Self::pics_depots_value(&vdf, appid);

            if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
                for (key, value) in depots.iter() {
                    if let Ok(d_id) = key.parse::<u64>() {
                        let m_id = extract_manifest_id_robust(value, branch).or_else(|| {
                            (branch != "public").then(|| extract_manifest_id_robust(value, "public")).flatten()
                        });
                        if let Some(m_id) = m_id {
                            manifests.insert(d_id, m_id);
                        }
                    }
                }
            }
        }
        Ok(manifests)
    }

    /// Fetch the current build id for a branch from PICS, for recording in the
    /// appmanifest so Steam treats the install as up to date. Falls back to `public`.
    pub(crate) async fn remote_buildid_static(
        connection: &Connection,
        appid: u32,
        branch: &str,
    ) -> Option<String> {
        let buffer = Self::pics_app_buffer(connection, appid).await.ok()?;
        let vdf = find_vdf_in_pics(&buffer).ok()?;
        let depots_val = Self::pics_depots_value(&vdf, appid);

        let buildid = |b: &str| {
            depots_val
                .and_then(|d| d.get_obj(&["branches", b]))
                .and_then(|node| node.get("buildid"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };
        buildid(branch).or_else(|| buildid("public"))
    }

    pub async fn fetch_app_metadata(&self, appid: u32) -> Option<AppMetadata> {
        let url = format!("https://store.steampowered.com/api/appdetails?appids={appid}&filters=basic");
        let resp = reqwest::get(url).await.ok()?;
        let json: serde_json::Value = resp.json().await.ok()?;
        let data = json.get(appid.to_string())?.get("data")?;

        let name = data.get("name")?.as_str()?.to_string();
        let header_image = data
            .get("header_image")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Some(AppMetadata { name, header_image })
    }

    /// If `appid` is a DLC, return the base game's appid it depends on.
    /// Returns `None` for base games (or if the relationship can't be determined).
    /// Tries the authoritative PICS appinfo first, then the StoreBrowse service as
    /// a fallback (some DLC don't carry the parent reference in their PICS appinfo).
    /// Both sources go over the Steam CM connection — no storefront API.
    pub async fn resolve_dlc_parent(&self, appid: u32) -> Option<u32> {
        if let Some(base) = self.dlc_parent_from_pics(appid).await {
            return Some(base);
        }
        self.dlc_parent_from_store(appid).await
    }

    /// DLC → base-game lookup via `StoreBrowse.GetItems` (`related_items.parent_appid`).
    pub(crate) async fn dlc_parent_from_store(&self, appid: u32) -> Option<u32> {
        let connection = self.connection.as_ref()?;

        let mut context = StoreBrowseContext::new();
        context.set_language("english".to_string());
        context.set_country_code("US".to_string());

        let mut request = CStoreBrowse_GetItems_Request::new();
        request.context = MessageField::some(context);
        let mut item_id = StoreItemID::new();
        item_id.set_appid(appid);
        request.ids.push(item_id);

        let response: CStoreBrowse_GetItems_Response = connection.service_method(request).await.ok()?;
        let item = response.store_items.iter().find(|i| i.appid() == appid)?;
        item.related_items
            .as_ref()
            .map(|r| r.parent_appid())
            .filter(|&base| base != 0 && base != appid)
    }

    pub(crate) async fn dlc_parent_from_pics(&self, appid: u32) -> Option<u32> {
        let connection = self.connection.as_ref()?;

        let buffer = Self::pics_app_buffer(connection, appid).await.ok()?;
        let raw_vdf = String::from_utf8(buffer).ok()?;
        let parsed = parse_appinfo(&raw_vdf).ok()?;

        let common = appinfo_common(&parsed);

        let is_dlc = common
            .and_then(|c| c.app_type.as_deref())
            .map(|t| t.eq_ignore_ascii_case("dlc"))
            .unwrap_or(false);
        if !is_dlc {
            return None;
        }

        let extended = parsed
            .appinfo
            .as_ref()
            .and_then(|a| a.extended.as_ref())
            .or(parsed.extended.as_ref());

        extended
            .and_then(|e| e.dependantonapp.as_deref())
            .and_then(|s| s.trim().parse::<u32>().ok())
            .or_else(|| {
                common
                    .and_then(|c| c.parent.as_deref())
                    .and_then(|s| s.trim().parse::<u32>().ok())
            })
            .filter(|&base| base != 0 && base != appid)
    }

    pub async fn resolve_install_game_info(&self, appid: u32) -> (String, Option<String>) {
        let mut display_name = format!("App {appid}");
        let mut installdir = None;

        // Try to get info from PICS first as it's authoritative
        if let Some(conn) = self.connection.as_ref() {
            let names = match Self::pics_app_buffer(conn, appid).await {
                Ok(buffer) => String::from_utf8(buffer)
                    .ok()
                    .and_then(|raw_vdf| parse_appinfo(&raw_vdf).ok())
                    .and_then(|parsed| {
                        let common = appinfo_common(&parsed)?;
                        Some((common.name.clone(), common.installdir.clone()))
                    }),
                Err(_) => None,
            };
            if let Some((name, dir)) = names {
                if let Some(name) = name {
                    display_name = name;
                }
                if dir.is_some() {
                    installdir = dir;
                }
            }
        }

        if installdir.is_none() || display_name.starts_with("App ") {
            if let Ok(games) = load_library_cache().await {
                if let Some(game) = games.iter().find(|g| g.app_id == appid) {
                    if display_name.starts_with("App ") && !game.name.is_empty() && !game.name.starts_with("App ") {
                        display_name = game.name.clone();
                    }
                }
            }
        }

        (display_name, installdir)
    }

    pub(crate) async fn resolve_install_game_name(&self, appid: u32) -> String {
        self.resolve_install_game_info(appid).await.0
    }

    /// Write a Steam `appmanifest_<appid>.acf` that the desktop client recognises as a
    /// complete, up-to-date install — so opening the Steam launcher does **not** treat
    /// the game as out-of-date and re-download over the files we just wrote.
    ///
    /// The two fields that resolve that clash are `buildid` (Steam compares it against
    /// the latest build in PICS; a missing/zero value reads as "update available") and
    /// `StateFlags`. When `fully_installed` is false the manifest is written in the
    /// "update required" state (4 → fully installed, 2 → update required) so an install
    /// in progress is registered with Steam rather than appearing as a fresh download.
    pub fn write_appmanifest(
        path: &Path,
        appid: u32,
        game_name: &str,
        installdir: &str,
        installed_depots: Vec<(u32, u64, u64)>,
        buildid: Option<&str>,
        fully_installed: bool,
    ) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed creating {}", parent.display()))?;
        }

        // Strip characters that are structurally significant in VDF text so a name
        // cannot break out of its quoted value or inject extra keys/blocks.
        let game_name = game_name.replace(['"', '\n', '\r', '{', '}', '\\'], "");
        let buildid = buildid.unwrap_or("0");
        let size_on_disk: u64 = installed_depots.iter().map(|(_, _, size)| *size).sum();
        // 4 = StateFullyInstalled, 2 = StateUpdateRequired.
        let state_flags = if fully_installed { 4 } else { 2 };
        let bytes_have = if fully_installed { size_on_disk } else { 0 };
        let last_updated = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut content = format!(
            "\"AppState\"\n{{\n\
             \t\"appid\"\t\t\"{appid}\"\n\
             \t\"Universe\"\t\t\"1\"\n\
             \t\"name\"\t\t\"{game_name}\"\n\
             \t\"StateFlags\"\t\t\"{state_flags}\"\n\
             \t\"installdir\"\t\t\"{installdir}\"\n\
             \t\"LastUpdated\"\t\t\"{last_updated}\"\n\
             \t\"SizeOnDisk\"\t\t\"{size_on_disk}\"\n\
             \t\"StagingSize\"\t\t\"0\"\n\
             \t\"buildid\"\t\t\"{buildid}\"\n\
             \t\"LastOwner\"\t\t\"0\"\n\
             \t\"UpdateResult\"\t\t\"0\"\n\
             \t\"BytesToDownload\"\t\t\"{size_on_disk}\"\n\
             \t\"BytesDownloaded\"\t\t\"{bytes_have}\"\n\
             \t\"BytesToStage\"\t\t\"{size_on_disk}\"\n\
             \t\"BytesStaged\"\t\t\"{bytes_have}\"\n\
             \t\"AutoUpdateBehavior\"\t\t\"0\"\n\
             \t\"AllowOtherDownloadsWhileRunning\"\t\t\"0\"\n\
             \t\"ScheduledAutoUpdate\"\t\t\"0\"\n"
        );

        if !installed_depots.is_empty() {
            content.push_str("\t\"InstalledDepots\"\n\t{\n");
            for (depot_id, manifest_id, size) in installed_depots {
                content.push_str(&format!(
                    "\t\t\"{depot_id}\"\n\t\t{{\n\t\t\t\"manifest\"\t\t\"{manifest_id}\"\n\t\t\t\"size\"\t\t\"{size}\"\n\t\t}}\n"
                ));
            }
            content.push_str("\t}\n");
        }

        content.push_str("}\n");

        std::fs::write(path, content)
            .with_context(|| format!("failed writing {}", path.display()))?;
        Ok(())
    }

    /// Mark a DLC as installed and enabled in the base game's appmanifest:
    ///
    /// 1. Add the DLC's downloaded depots to `InstalledDepots`, tagged with `dlcappid`
    ///    (how Steam records DLC content as present).
    /// 2. Remove the DLC's appid from every `DisabledDLC` list (how Steam records the
    ///    DLC as enabled vs. disabled).
    ///
    /// Existing depot entries are left untouched, so re-installs are idempotent.
    pub fn enable_dlc_in_appmanifest(
        base_manifest: &Path,
        dlc_appid: u32,
        depots: &[(u32, u64, u64)],
    ) -> Result<()> {
        let mut content = std::fs::read_to_string(base_manifest)
            .with_context(|| format!("failed reading {}", base_manifest.display()))?;
        let mut changed = false;

        // 1. Ensure the DLC's depots are recorded with their dlcappid.
        let mut entries = String::new();
        for (depot_id, manifest_id, size) in depots {
            if content.contains(&format!("\"{depot_id}\"")) {
                continue; // already recorded
            }
            entries.push_str(&format!(
                "\t\t\"{depot_id}\"\n\t\t{{\n\t\t\t\"manifest\"\t\t\"{manifest_id}\"\n\t\t\t\"size\"\t\t\"{size}\"\n\t\t\t\"dlcappid\"\t\t\"{dlc_appid}\"\n\t\t}}\n"
            ));
        }
        if !entries.is_empty() {
            if let Some(pos) = content.find("\"InstalledDepots\"") {
                let rel = content[pos..].find('{').ok_or_else(|| {
                    anyhow!("malformed InstalledDepots block in {}", base_manifest.display())
                })?;
                let insert_at = pos + rel + 1;
                content.insert_str(insert_at, &format!("\n{entries}"));
            } else {
                let last = content
                    .rfind('}')
                    .ok_or_else(|| anyhow!("malformed appmanifest {}", base_manifest.display()))?;
                content.insert_str(last, &format!("\t\"InstalledDepots\"\n\t{{\n{entries}\t}}\n"));
            }
            changed = true;
        }

        // 2. Remove the DLC appid from any "DisabledDLC" lists (enable it).
        let dlc_str = dlc_appid.to_string();
        let re = regex::Regex::new(r#""DisabledDLC"(\s*)"([^"]*)""#)
            .expect("valid DisabledDLC regex");
        let updated = re.replace_all(&content, |caps: &regex::Captures| {
            let kept: Vec<&str> = caps[2]
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty() && *s != dlc_str)
                .collect();
            format!("\"DisabledDLC\"{}\"{}\"", &caps[1], kept.join(","))
        });
        if updated != content {
            content = updated.into_owned();
            changed = true;
        }

        if changed {
            std::fs::write(base_manifest, &content)
                .with_context(|| format!("failed writing {}", base_manifest.display()))?;
            tracing::info!("Enabled DLC {dlc_appid} in {}", base_manifest.display());
        } else {
            tracing::info!("DLC {dlc_appid} already enabled in {}", base_manifest.display());
        }
        Ok(())
    }

    /// Enable or disable an owned DLC by editing the base game's appmanifest
    /// `DisabledDLC` lists. Returns the base game's appid.
    ///
    /// Note: the running Steam client is authoritative for DLC enable/disable and may
    /// rewrite this state on launch; this edits the on-disk manifest only.
    pub async fn set_dlc_enabled(&self, dlc_appid: u32, enabled: bool) -> Result<u32> {
        let base_appid = self.resolve_dlc_parent(dlc_appid).await.ok_or_else(|| {
            anyhow!("app {dlc_appid} is not a DLC, or its base game could not be determined")
        })?;

        let manifest = self.appmanifest_path(base_appid).await?;
        if !manifest.exists() {
            bail!("base game (app {base_appid}) for DLC {dlc_appid} is not installed");
        }

        let content = std::fs::read_to_string(&manifest)
            .with_context(|| format!("failed reading {}", manifest.display()))?;
        let updated = apply_dlc_disabled(&content, dlc_appid, !enabled);
        if updated != content {
            std::fs::write(&manifest, &updated)
                .with_context(|| format!("failed writing {}", manifest.display()))?;
        }
        tracing::info!(
            "{} DLC {dlc_appid} in {}",
            if enabled { "Enabled" } else { "Disabled" },
            manifest.display()
        );
        Ok(base_appid)
    }

    /// Resolve the ownership / install / enable status of each DLC of a base game.
    ///
    /// `owned` comes from the account (an app ownership ticket is issued only for
    /// licensed apps). `installed` and `disabled` are read from the base game's
    /// appmanifest — if the base game isn't installed, both are `false` for every DLC.
    pub async fn dlc_states(&self, base_appid: u32, dlc_ids: &[u32]) -> Result<Vec<DlcState>> {
        // Local install/enable state lives in the base game's appmanifest.
        let (installed, disabled) = match self.appmanifest_path(base_appid).await {
            Ok(path) if path.exists() => {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("failed reading {}", path.display()))?;
                (
                    parse_installed_dlc_appids(&content),
                    parse_disabled_dlc_appids(&content),
                )
            }
            _ => (HashSet::new(), HashSet::new()),
        };

        let mut out = Vec::with_capacity(dlc_ids.len());
        for &dlc_id in dlc_ids {
            // An ownership ticket is only issued for apps the account is licensed for.
            let owned = self.get_app_ticket(dlc_id).await.is_ok();
            out.push(DlcState {
                app_id: dlc_id,
                owned,
                installed: installed.contains(&dlc_id),
                disabled: disabled.contains(&dlc_id),
            });
        }
        Ok(out)
    }

}

/// Resolve an app's PICS `common` section, accounting for both appinfo layouts:
/// either nested under an `appinfo` wrapper node or present at the root.
fn appinfo_common(parsed: &crate::models::AppInfoRoot) -> Option<&crate::models::CommonNode> {
    parsed
        .appinfo
        .as_ref()
        .and_then(|a| a.common.as_ref())
        .or(parsed.common.as_ref())
}
