//! Depot metadata, shared-depot resolution, and depot-specific `SteamClient` methods.
use super::*;
use std::future::Future;
use steam_vdf_parser::{Obj, Value, Vdf};

#[derive(Debug)]
pub(crate) struct DepotKeyError {
    depot_id: u32,
    eresult: i32,
}

impl std::fmt::Display for DepotKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "failed to get depot key for depot {}: eresult {}",
            self.depot_id, self.eresult
        )
    }
}

impl std::error::Error for DepotKeyError {}

/// Does a key failure skip it?
///
/// Access denied skips any automatic depot.
/// Optional depots skip on any refusal.
pub(crate) fn skip_key_failure(error: &anyhow::Error, is_override: bool, required: bool) -> bool {
    !is_override
        && error
            .downcast_ref::<DepotKeyError>()
            .is_some_and(|e| e.eresult == 15 || !required)
}

/// Installed DLC depots are optional.
pub(crate) fn mark_dlc_optional(selections: &mut [ManifestSelection], dlc_depots: &HashSet<u64>) {
    for selection in selections {
        if dlc_depots.contains(&(selection.depot_id as u64)) {
            selection.required = false;
        }
    }
}

pub(crate) fn has_installable_content(
    successful: usize,
    required_selected: bool,
    required_successful: usize,
) -> bool {
    successful > 0 && (!required_selected || required_successful > 0)
}

pub(crate) struct ResolvedDepots {
    pub rows: Obj<'static>,
    sources: HashMap<u32, u32>,
    app_id: u32,
}

impl ResolvedDepots {
    pub fn source_app(&self, depot_id: u32) -> u32 {
        self.sources.get(&depot_id).copied().unwrap_or(self.app_id)
    }

    pub fn manifest_ids(&self, branch: &str) -> HashMap<u64, u64> {
        self.rows
            .iter()
            .filter_map(|(key, value)| {
                let depot_id = key.parse().ok()?;
                let manifest_id = extract_manifest_id_robust(value, branch).or_else(|| {
                    (branch != "public")
                        .then(|| extract_manifest_id_robust(value, "public"))
                        .flatten()
                })?;
                Some((depot_id, manifest_id))
            })
            .collect()
    }

    pub fn download_selections(
        &self,
        installed: &HashMap<u64, u64>,
        branch: &str,
        verify_mode: bool,
    ) -> Vec<ManifestSelection> {
        let manifests = if verify_mode {
            // Verification must keep installed (possibly pinned) manifest versions.
            installed.clone()
        } else {
            let mut manifests = self.manifest_ids(branch);
            if !installed.is_empty() {
                manifests.retain(|depot_id, _| installed.contains_key(depot_id));
            }
            manifests
        };
        manifests
            .into_iter()
            .map(|(depot_id, manifest_id)| ManifestSelection {
                app_id: self.source_app(depot_id as u32),
                depot_id: depot_id as u32,
                manifest_id,
                appinfo_vdf: String::new(),
                required: self
                    .rows
                    .get(&depot_id.to_string())
                    .is_none_or(|row| required_depot(row, self.app_id)),
            })
            .collect()
    }
}

/// Every selected manifest is already installed.
pub(crate) fn selections_match_installed(
    selections: &[ManifestSelection],
    installed: &HashMap<u64, u64>,
) -> bool {
    !selections.is_empty()
        && selections
            .iter()
            .all(|s| installed.get(&(s.depot_id as u64)) == Some(&s.manifest_id))
}

/// A depot row's `config/language`, if set.
pub(crate) fn depot_language<'a>(value: &'a Value<'_>) -> Option<&'a str> {
    value
        .get_obj(&["config"])
        .and_then(|c| c.get("language"))
        .and_then(|l| l.as_str())
        .map(str::trim)
        .filter(|l| !l.is_empty())
}

/// Language for `UserConfig`, like Steam.
///
/// Only a real language depot choice;
/// else Steam's client language applies.
pub(crate) fn recorded_language(
    rows: &Obj<'_>,
    platform: DepotPlatform,
    wanted: &str,
) -> Option<String> {
    let available = rows.values().any(|row| {
        let oslist = row
            .get_obj(&["config"])
            .and_then(|c| c.get("oslist"))
            .and_then(|o| o.as_str());
        should_keep_depot(oslist, platform)
            && depot_language(row).is_some_and(|l| l.eq_ignore_ascii_case(wanted))
    });
    available.then(|| wanted.to_ascii_lowercase())
}

/// Whether a depot serves `language`.
pub(crate) fn depot_matches_language(value: &Value<'_>, language: &str) -> bool {
    depot_language(value).is_none_or(|l| l.eq_ignore_ascii_case(language))
}

/// DLC app owning a depot.
pub(crate) fn dlc_app(value: &Value<'_>, app_id: u32) -> Option<u32> {
    number(value.get("dlcappid")).filter(|&id| id != 0 && id != app_id)
}

pub(crate) fn required_depot(value: &Value<'_>, app_id: u32) -> bool {
    number(value.get("dlcappid")).is_none_or(|id| id == 0 || id == app_id)
}

fn number(value: Option<&Value<'_>>) -> Option<u32> {
    let value = value?;
    value
        .as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| value.as_u64().and_then(|n| u32::try_from(n).ok()))
}

pub(crate) async fn resolve(
    connection: &Connection,
    app_id: u32,
    vdf: Vdf<'static>,
) -> Result<ResolvedDepots> {
    resolve_selected(connection, app_id, vdf, None).await
}

pub(crate) async fn fetch(
    connection: &Connection,
    app_id: u32,
    selected: Option<&HashMap<u64, u64>>,
) -> Result<ResolvedDepots> {
    let buffer = pics_app_buffer(connection, app_id, "failed requesting depot metadata").await?;
    let vdf = find_vdf_in_pics(&buffer).context("failed to parse depot metadata")?;
    resolve_selected(connection, app_id, vdf, selected).await
}

async fn resolve_selected(
    connection: &Connection,
    app_id: u32,
    vdf: Vdf<'static>,
    selected: Option<&HashMap<u64, u64>>,
) -> Result<ResolvedDepots> {
    resolve_with(app_id, vdf, selected, |source_app| async move {
        let buffer = pics_app_buffer(
            connection,
            source_app,
            "failed requesting shared depot appinfo",
        )
        .await?;
        find_vdf_in_pics(&buffer)
    })
    .await
}

async fn resolve_with<F, Fut>(
    app_id: u32,
    vdf: Vdf<'static>,
    selected: Option<&HashMap<u64, u64>>,
    fetch: F,
) -> Result<ResolvedDepots>
where
    F: FnMut(u32) -> Fut + Send,
    Fut: Future<Output = Result<Vdf<'static>>> + Send,
{
    let mut rows = pics_depots_value(&vdf)
        .and_then(Value::as_obj)
        .cloned()
        .context("appinfo has no depots object")?;
    // Uninstalled shared depots must not block updating or verifying this copy.
    let ids: Vec<u32> = rows
        .keys()
        .filter_map(|key| key.parse().ok())
        .filter(|id| selected.is_none_or(|depots| depots.contains_key(&u64::from(*id))))
        .collect();
    let mut resolver = Resolver {
        apps: HashMap::from([(app_id, vdf)]),
        fetch,
        allow_missing_manifests: selected.is_some(),
    };
    let mut sources = HashMap::new();
    for depot_id in ids {
        let original = rows.get(&depot_id.to_string()).unwrap();
        // sharedinstall=1 belongs to a separate installation (e.g. Common Redists),
        // not to this game's directory. Prerequisite installation is a separate step.
        if original.get("sharedinstall").and_then(Value::as_str) == Some("1") {
            continue;
        }
        let optional = !required_depot(original, app_id);
        match resolver.entry(app_id, depot_id, Vec::new()).await {
            Ok((row, source)) => {
                rows.insert(depot_id.to_string(), row);
                sources.insert(depot_id, source);
            }
            Err(error) if optional && selected.is_none() => {
                tracing::warn!(
                    depot_id,
                    "Optional DLC depot metadata unavailable: {error:#}"
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(ResolvedDepots {
        rows,
        sources,
        app_id,
    })
}

struct Resolver<F> {
    apps: HashMap<u32, Vdf<'static>>,
    fetch: F,
    allow_missing_manifests: bool,
}

impl<F, Fut> Resolver<F>
where
    F: FnMut(u32) -> Fut + Send,
    Fut: Future<Output = Result<Vdf<'static>>> + Send,
{
    async fn entry(
        &mut self,
        app_id: u32,
        depot_id: u32,
        mut path: Vec<(u32, u32)>,
    ) -> Result<(Value<'static>, u32)> {
        if path.len() >= 16 || path.contains(&(app_id, depot_id)) {
            bail!("shared depot reference cycle or depth limit for app {app_id} depot {depot_id}");
        }
        path.push((app_id, depot_id));
        if !self.apps.contains_key(&app_id) {
            let vdf = (self.fetch)(app_id).await.with_context(|| {
                format!("failed resolving shared depot {depot_id} from app {app_id}")
            })?;
            self.apps.insert(app_id, vdf);
        }
        let mut row = pics_depots_value(&self.apps[&app_id])
            .and_then(Value::as_obj)
            .and_then(|depots| depots.get(&depot_id.to_string()))
            .cloned()
            .with_context(|| format!("app {app_id} does not advertise shared depot {depot_id}"))?;
        let Some(source) = number(row.get("depotfromapp")) else {
            return Ok((row, app_id));
        };
        if source == 0 {
            bail!("invalid source app for shared depot {depot_id}");
        }
        // A proxy can supply its own branch manifests. Keep those rather than
        // substituting the parent's branch, but use the containing app for CDN auth.
        if row.get("manifests").is_some() {
            return Ok((row, source));
        }
        let (parent, containing_app) = Box::pin(self.entry(source, depot_id, path)).await?;
        let parent = parent
            .as_obj()
            .context("shared depot metadata is not an object")?;
        let child = row
            .as_obj_mut()
            .context("shared depot reference is not an object")?;
        for (key, value) in parent.iter() {
            if !child.contains_key(key) {
                child.insert(key.clone(), value.clone());
            }
        }
        // Installed versions remain verifiable after current manifests disappear.
        if child.get("manifests").is_none() && !self.allow_missing_manifests {
            bail!("shared depot {depot_id} from app {containing_app} has no manifests");
        }
        Ok((row, containing_app))
    }
}

impl SteamClient {
    pub async fn get_depot_list(&self, app_id: u32) -> Result<Vec<DepotInfo>> {
        let buffer = self
            .pics_buffer(
                app_id,
                "failed requesting appinfo product info for depot list",
            )
            .await?;

        let mut out = Vec::new();
        if let Ok(vdf) = find_vdf_in_pics(&buffer) {
            vdf.as_obj().context("root is not an object")?;
            let depots_val = pics_depots_value(&vdf);

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
            .pics_buffer(
                app_id,
                "failed requesting appinfo product info for manifests",
            )
            .await?;

        // Parse a gid value that may be encoded as a quoted string or a raw u64.
        fn parse_gid(v: &steam_vdf_parser::Value) -> Option<u64> {
            if let Some(s) = v.as_str() {
                return s.parse::<u64>().ok();
            }
            v.as_u64()
        }

        let resolved = depots::resolve(
            self.require_connection()?,
            app_id,
            find_vdf_in_pics(&buffer)?,
        )
        .await?;
        let resolved_value = steam_vdf_parser::Value::Obj(resolved.rows);
        let mut out = Vec::new();
        {
            let depots_val = Some(&resolved_value);
            if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
                for (key, value) in depots.iter() {
                    let Ok(depot_id) = key.parse::<u32>() else {
                        continue;
                    };
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
            .pics_buffer(
                app_id,
                "failed requesting appinfo product info for size estimate",
            )
            .await?;

        let mut est = InstallSizeEstimate::default();
        let vdf = find_vdf_in_pics(&buffer).context("failed to parse product info VDF")?;
        vdf.as_obj().context("root is not an object")?;
        let resolved = depots::resolve(self.require_connection()?, app_id, vdf).await?;
        let resolved_value = steam_vdf_parser::Value::Obj(resolved.rows);
        let depots_val = Some(&resolved_value);

        if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
            for (_depot_id, obj, public) in platform_depot_rows(depots, platform) {
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

    pub async fn get_depot_key(&self, app_id: u32, depot_id: u32) -> Result<Vec<u8>> {
        let connection = self.require_connection()?;
        let mut request = CMsgClientGetDepotDecryptionKey::new();
        request.set_depot_id(depot_id);
        request.set_app_id(app_id);

        let response: CMsgClientGetDepotDecryptionKeyResponse = connection.job(request).await?;
        if response.eresult() != 1 {
            return Err(DepotKeyError {
                depot_id,
                eresult: response.eresult(),
            }
            .into());
        }

        Ok(response.depot_encryption_key().to_vec())
    }

    pub async fn fetch_depots(&self, appid: u32) -> Result<Vec<BrowserDepotInfo>> {
        let connection = self.require_connection()?;
        depot_browser::fetch_depots(connection, appid).await
    }

    pub async fn fetch_manifest_files(
        &self,
        appid: u32,
        depot_id: u32,
        manifest_ref: &str,
    ) -> Result<Vec<ManifestFileEntry>> {
        let connection = self.require_connection()?;
        let cdn = self.browser_cdn_client(appid, depot_id).await?;
        depot_browser::fetch_manifest_files(connection, &cdn, appid, depot_id, manifest_ref).await
    }

    pub fn download_single_file(
        &self,
        appid: u32,
        depot_id: u32,
        manifest_ref: &str,
        file_path: &str,
        output_dir: &Path,
    ) -> Result<()> {
        let connection = self.require_connection()?;
        let runtime = tokio::runtime::Runtime::new()?;
        let cdn = runtime.block_on(self.browser_cdn_client(appid, depot_id))?;
        depot_browser::download_single_file(
            &runtime,
            connection,
            &cdn,
            depot_browser::DepotFileRef {
                appid,
                depot_id,
                manifest_ref,
                file_path,
            },
            output_dir,
        )
    }
}

#[cfg(test)]
mod tests;
