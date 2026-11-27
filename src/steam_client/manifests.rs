//! `SteamClient` methods: appmanifest paths, install-dir resolution, manifest writing, DLC state.
//!
//! Shares the parent's imports via `use super::*`.
use super::*;
use crate::core::acf;

/// Fields of one appmanifest.
#[derive(Clone, Debug, Default)]
pub struct AppManifestSpec<'a> {
    pub appid: u32,
    pub game_name: &'a str,
    pub installdir: &'a str,
    /// (depot, manifest, size) triples.
    pub installed_depots: Vec<(u32, u64, u64)>,
    pub buildid: Option<&'a str>,
    pub fully_installed: bool,
    pub pin_updates: bool,
    pub language: Option<&'a str>,
}

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

    pub(crate) async fn local_manifest_info_for_appid(
        &self,
        appid: u32,
    ) -> Result<(HashMap<u64, u64>, String)> {
        let manifest_path = self.appmanifest_path(appid).await?;
        if !manifest_path.exists() {
            return Ok((HashMap::new(), "public".to_string()));
        }
        let raw = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed reading {}", manifest_path.display()))?;
        let manifests = acf::installed_depots(&raw);
        let branch = acf::parse_app_manifest(&raw).active_branch;
        Ok((manifests, branch))
    }

    /// The depot → manifest ids currently recorded as installed in a game's local
    /// appmanifest (`InstalledDepots`). Empty when the game isn't installed. Used by
    /// `aurelia pin` to snapshot what version the pin should hold the game at.
    pub async fn installed_depot_manifests(&self, appid: u32) -> Result<HashMap<u32, u64>> {
        let (manifests, _branch) = self.local_manifest_info_for_appid(appid).await?;
        Ok(manifests.into_iter().map(|(d, m)| (d as u32, m)).collect())
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
            if let Some(installdir) = acf::parse_app_manifest(&raw).install_dir {
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
        Ok(
            PathBuf::from(load_launcher_config().await?.steam_library_path)
                .join("steamapps")
                .join("common")
                .join(appid.to_string()),
        )
    }

    pub(crate) fn probe_install_dir_by_appid(
        &self,
        steamapps: &Path,
        appid: u32,
    ) -> Option<PathBuf> {
        crate::library::probe_install_dir_by_appid(steamapps, appid)
    }

    pub(crate) async fn remote_manifest_ids_static(
        connection: &Connection,
        appid: u32,
        branch: &str,
    ) -> Result<HashMap<u64, u64>> {
        let resolved = depots::fetch(connection, appid, None).await?;
        Ok(resolved.manifest_ids(branch))
    }

    /// Fetch the current build id for a branch from PICS, for recording in the
    /// appmanifest so Steam treats the install as up to date. Falls back to `public`.
    pub(crate) async fn remote_buildid_static(
        connection: &Connection,
        appid: u32,
        branch: &str,
    ) -> Option<String> {
        let buffer = pics_app_buffer(
            connection,
            appid,
            "failed requesting appinfo product info for update metadata",
        )
        .await
        .ok()?;
        let vdf = find_vdf_in_pics(&buffer).ok()?;
        let depots_val = pics_depots_value(&vdf);

        let buildid = |b: &str| {
            depots_val
                .and_then(|d| d.get_obj(&["branches", b]))
                .and_then(|node| node.get("buildid"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };
        buildid(branch).or_else(|| buildid("public"))
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

        let response: CStoreBrowse_GetItems_Response =
            connection.service_method(request).await.ok()?;
        let item = response.store_items.iter().find(|i| i.appid() == appid)?;
        item.related_items
            .as_ref()
            .map(|r| r.parent_appid())
            .filter(|&base| base != 0 && base != appid)
    }

    pub(crate) async fn dlc_parent_from_pics(&self, appid: u32) -> Option<u32> {
        let connection = self.connection.as_ref()?;

        let buffer = pics_app_buffer(
            connection,
            appid,
            "failed requesting appinfo product info for update metadata",
        )
        .await
        .ok()?;
        let vdf = find_vdf_in_pics(&buffer).ok()?;
        let section = pics_app_section(vdf.value());

        let is_dlc = section
            .get_str(&["common", "type"])
            .map(|t| t.eq_ignore_ascii_case("dlc"))
            .unwrap_or(false);
        if !is_dlc {
            return None;
        }

        section
            .get_str(&["extended", "dependantonapp"])
            .and_then(|s| s.trim().parse::<u32>().ok())
            .or_else(|| {
                section
                    .get_str(&["common", "parent"])
                    .and_then(|s| s.trim().parse::<u32>().ok())
            })
            .filter(|&base| base != 0 && base != appid)
    }

    /// Resolve the display name and Steam install directory for `appid` from PICS
    pub async fn resolve_install_game_info(&self, appid: u32) -> (String, Option<String>) {
        let mut display_name = format!("App {appid}");
        let mut installdir = None;

        // PICS is authoritative for both the name and the install directory.
        if let Some(conn) = self.connection.as_ref() {
            match pics_app_buffer(
                conn,
                appid,
                "failed requesting appinfo product info for update metadata",
            )
            .await
            {
                Ok(buffer) => match find_vdf_in_pics(&buffer) {
                    Ok(vdf) => {
                        let section = pics_app_section(vdf.value());
                        if let Some(name) = section
                            .get_str(&["common", "name"])
                            .map(str::trim)
                            .filter(|n| !n.is_empty())
                        {
                            display_name = name.to_string();
                        }
                        installdir = pics_section_installdir(section);
                    }
                    Err(e) => tracing::warn!("could not parse PICS appinfo for app {appid}: {e:#}"),
                },
                Err(e) => tracing::warn!("could not fetch PICS appinfo for app {appid}: {e:#}"),
            }
        }

        if display_name.starts_with("App ")
            && let Ok(games) = load_library_cache().await
            && let Some(game) = games.iter().find(|g| g.app_id == appid)
            && !game.name.is_empty()
            && !game.name.starts_with("App ")
        {
            display_name = game.name.clone();
        }

        (display_name, installdir)
    }

    /// Retry PICS installdir with access token.
    pub(crate) async fn retry_pics_installdir(&self, appid: u32) -> Option<String> {
        let conn = self.connection.as_ref()?;
        let buffer = pics_app_buffer_with_token(
            conn,
            appid,
            "failed requesting appinfo product info for install dir",
        )
        .await
        .inspect_err(|e| tracing::warn!("PICS token retry failed for app {appid}: {e:#}"))
        .ok()?;
        let vdf = find_vdf_in_pics(&buffer).ok()?;
        pics_section_installdir(pics_app_section(vdf.value()))
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
    ///
    /// `pin_updates` writes `AutoUpdateBehavior "1"` ("only update on launch") instead
    /// of the default `"0"`, so the official Steam client won't eagerly re-patch a
    /// downgraded/pinned install out from under Aurelia. Normal installs pass `false`.
    ///
    /// Existing `UserConfig`/`MountedConfig` are copied verbatim.
    /// `language` fills a missing `UserConfig` language.
    pub fn write_appmanifest(path: &Path, spec: AppManifestSpec<'_>) -> Result<()> {
        let AppManifestSpec {
            appid,
            game_name,
            installdir,
            installed_depots,
            buildid,
            fully_installed,
            pin_updates,
            language,
        } = spec;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed creating {}", parent.display()))?;
        }

        // Preserve existing manifest
        let existing = std::fs::read_to_string(path).ok();
        let (last_owner, beta_key) = match existing.as_deref() {
            Some(existing) => {
                let parsed = acf::parse_app_manifest(existing);
                (
                    parsed.last_owner.unwrap_or(0),
                    Some(parsed.active_branch).filter(|b| b != "public"),
                )
            }
            None => (0, None),
        };
        let old_user_config = existing
            .as_deref()
            .and_then(|raw| top_level_block(raw, "UserConfig"));
        let mounted_config = existing
            .as_deref()
            .and_then(|raw| top_level_block(raw, "MountedConfig"));
        let kept = existing
            .as_deref()
            .map(|raw| kept_fields(raw, fully_installed))
            .unwrap_or_default();
        let old_depots = existing
            .as_deref()
            .and_then(|raw| top_level_block(raw, "InstalledDepots"))
            .map(depot_entries)
            .unwrap_or_default();

        // Strip characters that are structurally significant in VDF text so a name
        // cannot break out of its quoted value or inject extra keys/blocks.
        let game_name = game_name.replace(['"', '\n', '\r', '{', '}', '\\'], "");
        let buildid = buildid.unwrap_or("0");
        // DLC content stays on disk.
        let carried: Vec<&DepotEntry> = old_depots
            .iter()
            .filter(|d| d.is_dlc() && !installed_depots.iter().any(|(id, ..)| *id == d.id))
            .collect();
        let size_on_disk: u64 = installed_depots
            .iter()
            .map(|(_, _, size)| *size)
            .sum::<u64>()
            + carried.iter().map(|d| d.size).sum::<u64>();
        // 4 = StateFullyInstalled, 2 = StateUpdateRequired.
        let state_flags = if fully_installed { 4 } else { 2 };
        // 0 = auto-update always, 1 = only update on launch (used to hold a pinned/
        // downgraded build in place against the official client's eager re-patching).
        let auto_update_behavior = if pin_updates { 1 } else { 0 };
        let bytes_have = if fully_installed { size_on_disk } else { 0 };
        let last_updated = crate::core::time::now_unix();

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
             \t\"LastOwner\"\t\t\"{last_owner}\"\n\
             \t\"UpdateResult\"\t\t\"0\"\n\
             \t\"BytesToDownload\"\t\t\"{size_on_disk}\"\n\
             \t\"BytesDownloaded\"\t\t\"{bytes_have}\"\n\
             \t\"BytesToStage\"\t\t\"{size_on_disk}\"\n\
             \t\"BytesStaged\"\t\t\"{bytes_have}\"\n\
             \t\"AutoUpdateBehavior\"\t\t\"{auto_update_behavior}\"\n\
             \t\"AllowOtherDownloadsWhileRunning\"\t\t\"0\"\n\
             \t\"ScheduledAutoUpdate\"\t\t\"0\"\n"
        );
        content.push_str(&kept.pairs);

        if !installed_depots.is_empty() || !carried.is_empty() {
            content.push_str("\t\"InstalledDepots\"\n\t{\n");
            for (depot_id, manifest_id, size) in installed_depots {
                let extras = old_depots
                    .iter()
                    .find(|d| d.id == depot_id)
                    .map(|d| d.extras.as_str())
                    .unwrap_or("");
                content.push_str(&format!(
                    "\t\t\"{depot_id}\"\n\t\t{{\n\t\t\t\"manifest\"\t\t\"{manifest_id}\"\n\t\t\t\"size\"\t\t\"{size}\"\n{extras}\t\t}}\n"
                ));
            }
            for depot in carried {
                content.push_str(&format!("\t\t{}\n", depot.raw));
            }
            content.push_str("\t}\n");
        }

        let clean = |s: &str| s.replace(['"', '\n', '\r', '{', '}', '\\'], "");
        let user_config = match old_user_config {
            Some(block) => Some(block.to_string()),
            None => {
                let mut entries = String::new();
                if let Some(branch) = beta_key {
                    entries.push_str(&format!("\t\t\"betakey\"\t\t\"{}\"\n", clean(&branch)));
                }
                (!entries.is_empty()).then(|| format!("\"UserConfig\"\n\t{{\n{entries}\t}}"))
            }
        };
        let language = language.map(clean).filter(|l| !l.is_empty());
        let user_config = match (user_config, language) {
            (Some(block), Some(lang)) => Some(block_with_key(&block, "language", &lang)),
            (None, Some(lang)) => Some(format!(
                "\"UserConfig\"\n\t{{\n\t\t\"language\"\t\t\"{lang}\"\n\t}}"
            )),
            (block, None) => block,
        };
        for block in user_config.iter().map(String::as_str).chain(mounted_config) {
            content.push('\t');
            content.push_str(block);
            content.push('\n');
        }
        content.push_str(&kept.blocks);

        content.push_str("}\n");

        crate::core::atomic::write_atomic(path, content.as_bytes())?;
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
                    anyhow!(
                        "malformed InstalledDepots block in {}",
                        base_manifest.display()
                    )
                })?;
                let insert_at = pos + rel + 1;
                content.insert_str(insert_at, &format!("\n{entries}"));
            } else {
                let last = content
                    .rfind('}')
                    .ok_or_else(|| anyhow!("malformed appmanifest {}", base_manifest.display()))?;
                content.insert_str(
                    last,
                    &format!("\t\"InstalledDepots\"\n\t{{\n{entries}\t}}\n"),
                );
            }
            changed = true;
        }

        // 2. Remove the DLC appid from any "DisabledDLC" lists (enable it).
        let updated = apply_dlc_disabled(&content, dlc_appid, false);
        if updated != content {
            content = updated;
            changed = true;
        }

        if changed {
            crate::core::atomic::write_atomic(base_manifest, content.as_bytes())?;
            tracing::info!("Enabled DLC {dlc_appid} in {}", base_manifest.display());
        } else {
            tracing::info!(
                "DLC {dlc_appid} already enabled in {}",
                base_manifest.display()
            );
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
            crate::core::atomic::write_atomic(&manifest, updated.as_bytes())?;
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

        let owned = self.owned_among(dlc_ids).await?;
        Ok(dlc_ids
            .iter()
            .map(|&dlc_id| DlcState {
                app_id: dlc_id,
                owned: owned.contains(&dlc_id),
                installed: installed.contains(&dlc_id),
                disabled: disabled.contains(&dlc_id),
            })
            .collect())
    }

    /// Owned apps among `app_ids`, from one batched token request.
    ///
    /// Without a connection every app reads as not owned.
    async fn owned_among(&self, app_ids: &[u32]) -> Result<HashSet<u32>> {
        let Some(connection) = self.connection() else {
            return Ok(HashSet::new());
        };
        if app_ids.is_empty() {
            return Ok(HashSet::new());
        }
        super::pics::pics_access_grants(connection, app_ids).await
    }
}

/// One lexical token of VDF text.
#[derive(Clone, Copy)]
enum Tok<'a> {
    /// Quoted string, with its start offset.
    Str(usize, &'a str),
    Open,
    /// Closing brace offset.
    Close(usize),
}

/// Lex quoted strings and braces.
fn vdf_tokens(raw: &str) -> Vec<Tok<'_>> {
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    // Skip escaped characters.
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                let end = i.min(bytes.len());
                out.push(Tok::Str(start, raw.get(start + 1..end).unwrap_or("")));
            }
            b'{' => out.push(Tok::Open),
            b'}' => out.push(Tok::Close(i)),
            _ => {}
        }
        i += 1;
    }
    out
}

/// Raw top-level `name` block text.
pub(crate) fn top_level_block<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
    let toks = vdf_tokens(raw);
    let mut depth = 0usize;
    let mut start = None;
    for (idx, tok) in toks.iter().enumerate() {
        match *tok {
            Tok::Open => {
                if depth == 1
                    && start.is_none()
                    && let Some(Tok::Str(s, key)) = idx.checked_sub(1).map(|p| toks[p])
                    && key.eq_ignore_ascii_case(name)
                {
                    start = Some(s);
                }
                depth += 1;
            }
            Tok::Close(pos) => {
                depth = depth.checked_sub(1)?;
                if depth == 1
                    && let Some(s) = start
                {
                    return raw.get(s..=pos);
                }
            }
            Tok::Str(..) => {}
        }
    }
    None
}

/// A block's direct string pairs.
fn block_pairs(block: &str) -> Vec<(&str, &str)> {
    let toks = vdf_tokens(block);
    let mut pairs = Vec::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < toks.len() {
        match toks[i] {
            Tok::Open => depth += 1,
            Tok::Close(_) => depth = depth.saturating_sub(1),
            Tok::Str(_, key) if depth == 1 => {
                if let Some(Tok::Str(_, value)) = toks.get(i + 1) {
                    pairs.push((key, *value));
                    i += 1;
                }
            }
            Tok::Str(..) => {}
        }
        i += 1;
    }
    pairs
}

/// Add `key` unless already present.
fn block_with_key(block: &str, key: &str, value: &str) -> String {
    if block_pairs(block)
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case(key))
    {
        return block.to_string();
    }
    let Some(close) = block.rfind('}') else {
        return block.to_string();
    };
    let line = format!("\t\t\"{key}\"\t\t\"{value}\"\n");
    let mut out = block.to_string();
    match block[..close].rfind('\n') {
        Some(nl) => out.insert_str(nl + 1, &line),
        None => out.insert_str(close, &format!("\n{line}\t")),
    }
    out
}

/// Child blocks as `(key, start, end)`.
fn child_blocks(block: &str) -> Vec<(&str, usize, usize)> {
    let toks = vdf_tokens(block);
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut open: Option<(&str, usize)> = None;
    for (idx, tok) in toks.iter().enumerate() {
        match *tok {
            Tok::Open => {
                if depth == 1
                    && let Some(Tok::Str(s, key)) = idx.checked_sub(1).map(|p| toks[p])
                {
                    open = Some((key, s));
                }
                depth += 1;
            }
            Tok::Close(pos) => {
                depth = depth.saturating_sub(1);
                if depth == 1
                    && let Some((key, s)) = open.take()
                {
                    out.push((key, s, pos));
                }
            }
            Tok::Str(..) => {}
        }
    }
    out
}

/// Set or add a direct `key`.
fn block_set_key(block: &str, key: &str, value: &str) -> String {
    let toks = vdf_tokens(block);
    let mut depth = 0usize;
    let mut i = 0;
    while i < toks.len() {
        match toks[i] {
            Tok::Open => depth += 1,
            Tok::Close(_) => depth = depth.saturating_sub(1),
            Tok::Str(_, k) if depth == 1 => {
                if let Some(Tok::Str(vs, old)) = toks.get(i + 1) {
                    if k.eq_ignore_ascii_case(key) {
                        let mut out = block.to_string();
                        out.replace_range(*vs..vs + old.len() + 2, &format!("\"{value}\""));
                        return out;
                    }
                    i += 1;
                }
            }
            Tok::Str(..) => {}
        }
        i += 1;
    }
    block_with_key(block, key, value)
}

/// Byte range of a top-level block.
fn top_level_range(raw: &str, name: &str) -> Option<std::ops::Range<usize>> {
    let block = top_level_block(raw, name)?;
    let start = block.as_ptr() as usize - raw.as_ptr() as usize;
    Some(start..start + block.len())
}

/// Append a top-level block.
fn push_top_level(raw: &mut String, block: &str) {
    if let Some(last) = raw.rfind('}') {
        raw.insert_str(last, &format!("\t{block}\n"));
    }
}

/// Swap language depots and record `language`.
///
/// In-place text edit; keeps the rest.
pub(crate) fn switch_language_in_manifest(
    raw: &str,
    remove: &HashSet<u32>,
    add: &[(u32, u64, u64)],
    dlc: &HashMap<u32, u32>,
    language: &str,
) -> String {
    let mut out = raw.to_string();
    let new_entries: String = add
        .iter()
        .map(|(depot, manifest, size)| {
            let tag = dlc
                .get(depot)
                .map(|app| format!("\t\t\t\"dlcappid\"\t\t\"{app}\"\n"))
                .unwrap_or_default();
            format!(
                "\t\t\"{depot}\"\n\t\t{{\n\t\t\t\"manifest\"\t\t\"{manifest}\"\n\t\t\t\"size\"\t\t\"{size}\"\n{tag}\t\t}}\n"
            )
        })
        .collect();

    match top_level_range(&out, "InstalledDepots") {
        Some(range) => {
            let block = &out[range.clone()];
            let mut rebuilt = String::from("\"InstalledDepots\"\n\t{\n");
            for (key, s, e) in child_blocks(block) {
                let added = add.iter().any(|(d, ..)| d.to_string() == key);
                if key.parse::<u32>().is_ok_and(|d| remove.contains(&d)) || added {
                    continue;
                }
                rebuilt.push_str("\t\t");
                rebuilt.push_str(&block[s..=e]);
                rebuilt.push('\n');
            }
            rebuilt.push_str(&new_entries);
            rebuilt.push_str("\t}");
            out.replace_range(range, &rebuilt);
        }
        None if !add.is_empty() => {
            push_top_level(
                &mut out,
                &format!("\"InstalledDepots\"\n\t{{\n{new_entries}\t}}"),
            );
        }
        None => {}
    }

    // Steam sums InstalledDepots sizes.
    let size: u64 = top_level_block(&out, "InstalledDepots")
        .map(depot_entries)
        .unwrap_or_default()
        .iter()
        .map(|d| d.size)
        .sum();
    if block_pairs(&out)
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("SizeOnDisk"))
    {
        out = block_set_key(&out, "SizeOnDisk", &size.to_string());
    }

    match top_level_range(&out, "UserConfig") {
        Some(range) => {
            let updated = block_set_key(&out[range.clone()], "language", language);
            out.replace_range(range, &updated);
        }
        None => push_top_level(
            &mut out,
            &format!("\"UserConfig\"\n\t{{\n\t\t\"language\"\t\t\"{language}\"\n\t}}"),
        ),
    }

    if let Some(range) = top_level_range(&out, "MountedConfig") {
        let block = &out[range.clone()];
        if block_pairs(block)
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("language"))
        {
            let updated = block_set_key(block, "language", language);
            out.replace_range(range, &updated);
        }
    }
    out
}

/// Top-level keys `write_appmanifest` writes itself.
const OWNED_KEYS: &[&str] = &[
    "appid",
    "universe",
    "name",
    "stateflags",
    "installdir",
    "lastupdated",
    "sizeondisk",
    "stagingsize",
    "buildid",
    "lastowner",
    "updateresult",
    "bytestodownload",
    "bytesdownloaded",
    "bytestostage",
    "bytesstaged",
    "autoupdatebehavior",
    "allowotherdownloadswhilerunning",
    "scheduledautoupdate",
];

/// Top-level blocks written separately.
const OWNED_BLOCKS: &[&str] = &["installeddepots", "userconfig", "mountedconfig"];

/// Existing Steam fields to carry over.
#[derive(Default)]
struct KeptFields {
    /// Formatted `key value` lines.
    pairs: String,
    /// Formatted unknown blocks.
    blocks: String,
}

/// Fields Aurelia doesn't own, verbatim.
fn kept_fields(raw: &str, fully_installed: bool) -> KeptFields {
    let owned = |key: &str| {
        let key = key.to_ascii_lowercase();
        // A finished install has no target.
        OWNED_KEYS.contains(&key.as_str()) || (fully_installed && key == "targetbuildid")
    };
    let mut kept = KeptFields::default();
    for (key, value) in block_pairs(raw) {
        if !owned(key) {
            kept.pairs
                .push_str(&format!("\t\"{key}\"\t\t\"{value}\"\n"));
        }
    }
    if fully_installed
        && block_pairs(raw)
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("targetbuildid"))
    {
        kept.pairs.push_str("\t\"TargetBuildID\"\t\t\"0\"\n");
    }
    for (key, start, end) in child_blocks(raw) {
        if !OWNED_BLOCKS.contains(&key.to_ascii_lowercase().as_str()) {
            kept.blocks.push_str(&format!("\t{}\n", &raw[start..=end]));
        }
    }
    kept
}

/// One existing `InstalledDepots` entry.
struct DepotEntry {
    id: u32,
    /// Whole block text.
    raw: String,
    /// Lines besides manifest and size.
    extras: String,
    dlc: bool,
    size: u64,
}

impl DepotEntry {
    fn is_dlc(&self) -> bool {
        self.dlc
    }
}

/// Parse `InstalledDepots` entries.
fn depot_entries(block: &str) -> Vec<DepotEntry> {
    child_blocks(block)
        .into_iter()
        .filter_map(|(key, start, end)| {
            let id = key.parse().ok()?;
            let text = &block[start..=end];
            let mut extras = String::new();
            let mut dlc = false;
            let mut size = 0;
            for (k, v) in block_pairs(text) {
                let lower = k.to_ascii_lowercase();
                if lower == "size" {
                    size = v.parse().unwrap_or(0);
                    continue;
                }
                if lower == "manifest" {
                    continue;
                }
                dlc |= lower == "dlcappid";
                extras.push_str(&format!("\t\t\t\"{k}\"\t\t\"{v}\"\n"));
            }
            Some(DepotEntry {
                id,
                raw: text.to_string(),
                extras,
                dlc,
                size,
            })
        })
        .collect()
}

/// Installed depots tagged with `dlcappid`.
pub(crate) fn installed_dlc_depots(raw: &str) -> HashSet<u64> {
    top_level_block(raw, "InstalledDepots")
        .map(depot_entries)
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.is_dlc())
        .map(|d| u64::from(d.id))
        .collect()
}

/// Per-game `UserConfig/language`, if set.
pub(crate) fn user_config_language(raw: &str) -> Option<String> {
    let block = top_level_block(raw, "UserConfig")?;
    block_pairs(block)
        .into_iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("language"))
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Safe `config/installdir` from PICS section.
fn pics_section_installdir(section: &steam_vdf_parser::Value) -> Option<String> {
    section
        .get_str(&["config", "installdir"])
        .map(str::trim)
        .filter(|d| crate::core::paths::is_safe_dir_name(d))
        .map(str::to_string)
}
