//! App and DLC facts for games.
//!
//! DLC ids from PICS, names
//! from the store, install state
//! from appmanifests.

use super::*;

/// One DLC of an app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DlcInfo {
    pub app_id: u32,
    /// Store name; `None` without store page.
    pub name: Option<String>,
    /// Depots on disk, or none needed.
    pub content_ready: bool,
    /// Listed in `DisabledDLC`.
    pub disabled: bool,
}

/// Local install facts of an app.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppInstallInfo {
    pub build_id: u32,
    /// Set only when installed.
    pub install_dir: Option<PathBuf>,
    /// `UserConfig/language` from the appmanifest.
    pub language: Option<String>,
}

/// DLC facts kept for offline use.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DlcCatalog {
    pub ids: Vec<u32>,
    pub with_depots: Vec<u32>,
    pub names: HashMap<u32, String>,
}

/// Cached catalog file of `appid`.
pub fn dlc_catalog_path(dir: &Path, appid: u32) -> PathBuf {
    dir.join(format!("{appid}.json"))
}

/// Default catalog directory.
pub fn dlc_catalog_dir() -> Result<PathBuf> {
    Ok(crate::core::config::data_dir()?.join("steamworks-dlc"))
}

/// Save a catalog; best effort.
pub fn store_dlc_catalog(dir: &Path, appid: u32, catalog: &DlcCatalog) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_vec(catalog)?;
    crate::core::atomic::write_private(&dlc_catalog_path(dir, appid), &json)
}

/// Saved catalog, if readable.
pub fn load_dlc_catalog(dir: &Path, appid: u32) -> Option<DlcCatalog> {
    let raw = std::fs::read(dlc_catalog_path(dir, appid)).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// DLC ids owning a depot.
pub(crate) fn dlcs_with_depots(section: &steam_vdf_parser::Value) -> HashSet<u32> {
    let Some(depots) = section.get_obj(&["depots"]) else {
        return HashSet::new();
    };
    depots
        .values()
        .filter_map(|node| node.get("dlcappid")?.as_str()?.trim().parse().ok())
        .collect()
}

/// Joins PICS, store and manifest facts.
pub(crate) fn build_dlc_list(
    ids: &[u32],
    with_depots: &HashSet<u32>,
    installed: &HashSet<u32>,
    disabled: &HashSet<u32>,
    names: &HashMap<u32, String>,
) -> Vec<DlcInfo> {
    ids.iter()
        .map(|&id| DlcInfo {
            app_id: id,
            name: names.get(&id).filter(|n| !n.is_empty()).cloned(),
            content_ready: installed.contains(&id) || !with_depots.contains(&id),
            disabled: disabled.contains(&id),
        })
        .collect()
}

/// Languages an app supports, PICS order.
pub(crate) fn supported_languages(section: &steam_vdf_parser::Value) -> Vec<String> {
    let yes = |v: &str| matches!(v.trim(), "1" | "true");
    if let Some(list) = section.get_obj(&["common", "supported_languages"]) {
        return list
            .iter()
            .filter(|(_, v)| v.get("supported").and_then(|s| s.as_str()).is_some_and(yes))
            .map(|(k, _)| k.to_string())
            .collect();
    }
    // Older appinfo: flat flags.
    section
        .get_obj(&["common", "languages"])
        .map(|list| {
            list.iter()
                .filter(|(_, v)| v.as_str().is_some_and(yes))
                .map(|(k, _)| k.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Comma list; `current` when unknown.
pub fn languages_list(supported: &[String], current: &str) -> String {
    if supported.is_empty() {
        return current.to_string();
    }
    supported.join(",")
}

/// Lender of shared app, else us.
///
/// `None` when the lender is unknown.
pub fn owner_from_shared(app_id: u32, me: u64, shared: &[SharedApp]) -> Option<u64> {
    match shared.iter().find(|s| s.app_id == app_id) {
        Some(app) => app.owner_steamid.filter(|o| *o != 0),
        None => Some(me),
    }
}

impl SteamClient {
    /// Account whose license runs `appid`.
    pub async fn app_owner(&self, appid: u32) -> Result<Option<u64>> {
        let me = self.steam_id().context("not logged in")?;
        let shared = self.fetch_family_shared_apps().await?;
        Ok(owner_from_shared(appid, me, &shared))
    }

    /// Languages from PICS appinfo.
    pub async fn supported_game_languages(&self, appid: u32) -> Result<Vec<String>> {
        let buffer = self
            .pics_buffer(appid, "failed requesting appinfo for languages")
            .await?;
        let vdf = find_vdf_in_pics(&buffer).context("failed to parse product info VDF")?;
        Ok(supported_languages(pics_app_section(vdf.value())))
    }

    /// DLC of `appid` in PICS order.
    pub async fn steamworks_dlcs(&self, appid: u32, language: &str) -> Result<Vec<DlcInfo>> {
        let buffer = self
            .pics_buffer(appid, "failed requesting appinfo for DLC list")
            .await?;
        let (ids, with_depots) = {
            let vdf = find_vdf_in_pics(&buffer).context("failed to parse product info VDF")?;
            let section = pics_app_section(vdf.value());
            (dlc_ids_from_section(section), dlcs_with_depots(section))
        };
        let (installed, disabled) = self.manifest_dlc_state(appid).await?;
        let country = self.require_connection()?.ip_country_code().to_string();
        let names: HashMap<u32, String> = if ids.is_empty() {
            HashMap::new()
        } else {
            self.fetch_store_apps(&ids, language, &country)
                .await
                .context("failed fetching DLC store records")?
                .into_iter()
                .map(|app| (app.app_id, app.name))
                .collect()
        };
        let catalog = DlcCatalog {
            ids: ids.clone(),
            with_depots: with_depots.iter().copied().collect(),
            names: names.clone(),
        };
        let saved = dlc_catalog_dir().and_then(|dir| store_dlc_catalog(&dir, appid, &catalog));
        if let Err(e) = saved {
            tracing::debug!(appid, "not caching DLC catalog: {e:#}");
        }
        Ok(build_dlc_list(
            &ids,
            &with_depots,
            &installed,
            &disabled,
            &names,
        ))
    }

    /// Saved-catalog DLC; `None` without one.
    pub async fn offline_steamworks_dlcs(
        &self,
        appid: u32,
        dir: &Path,
    ) -> Result<Option<Vec<DlcInfo>>> {
        let Some(catalog) = load_dlc_catalog(dir, appid) else {
            return Ok(None);
        };
        let (installed, disabled) = self.manifest_dlc_state(appid).await?;
        let with_depots: HashSet<u32> = catalog.with_depots.iter().copied().collect();
        Ok(Some(build_dlc_list(
            &catalog.ids,
            &with_depots,
            &installed,
            &disabled,
            &catalog.names,
        )))
    }

    /// Installed and disabled DLC ids.
    async fn manifest_dlc_state(&self, appid: u32) -> Result<(HashSet<u32>, HashSet<u32>)> {
        match self.appmanifest_path(appid).await {
            Ok(path) if path.exists() => {
                let raw = std::fs::read_to_string(&path)
                    .with_context(|| format!("failed reading {}", path.display()))?;
                Ok((
                    parse_installed_dlc_appids(&raw),
                    parse_disabled_dlc_appids(&raw),
                ))
            }
            _ => Ok((HashSet::new(), HashSet::new())),
        }
    }

    /// Build, folder and language.
    pub async fn app_install_info(&self, appid: u32) -> Result<AppInstallInfo> {
        let path = self.appmanifest_path(appid).await?;
        if !path.exists() {
            return Ok(AppInstallInfo::default());
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("failed reading {}", path.display()))?;
        let manifest = crate::core::acf::parse_app_manifest(&raw);
        let dir = self.install_root_for_app(appid).await?;
        Ok(AppInstallInfo {
            build_id: u32::try_from(manifest.build_id).unwrap_or(0),
            install_dir: (manifest.fully_installed() && dir.is_dir()).then_some(dir),
            language: manifest.language,
        })
    }

    /// Country Steam saw at logon.
    pub fn ip_country(&self) -> Option<String> {
        let code = self.connection.as_ref()?.ip_country_code();
        (!code.is_empty()).then(|| code.to_string())
    }
}

#[cfg(test)]
#[path = "steamworks_info_tests.rs"]
mod tests;
