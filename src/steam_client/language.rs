//! Switch an installed game's language.
//!
//! Swaps language depots like Steam does.
use super::*;
use crate::core::acf;
use steam_vdf_parser::{Obj, Value};

/// What a language switch will change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LanguagePlan {
    /// Installed depots to drop.
    pub remove: Vec<(u32, u64)>,
    /// Depots to download, with manifests.
    pub add: Vec<(u32, u64)>,
    /// Installed depots that stay.
    pub keep: Vec<(u32, u64)>,
    /// Added depot to its DLC app.
    pub dlc: HashMap<u32, u32>,
}

/// Platform of the installed depots.
pub(crate) fn installed_platform(rows: &Obj<'_>, installed: &HashMap<u64, u64>) -> DepotPlatform {
    let linux = installed.keys().any(|depot| {
        rows.get(&depot.to_string())
            .and_then(|row| row.get_obj(&["config"]))
            .and_then(|c| c.get("oslist"))
            .and_then(|o| o.as_str())
            .is_some_and(|os| os.to_lowercase().contains("linux"))
    });
    if linux {
        DepotPlatform::Linux
    } else {
        DepotPlatform::Windows
    }
}

/// Language depots of the base game.
fn is_language_row(row: &Value<'_>, app_id: u32, platform: DepotPlatform) -> bool {
    let oslist = row
        .get_obj(&["config"])
        .and_then(|c| c.get("oslist"))
        .and_then(|o| o.as_str());
    depots::depot_language(row).is_some()
        && depots::required_depot(row, app_id)
        && should_keep_depot(oslist, platform)
}

/// Languages with their own depots.
pub(crate) fn depot_languages(rows: &Obj<'_>, app_id: u32, platform: DepotPlatform) -> Vec<String> {
    let mut langs: Vec<String> = rows
        .values()
        .filter(|row| is_language_row(row, app_id, platform))
        .filter_map(|row| depots::depot_language(row).map(str::to_ascii_lowercase))
        .collect();
    langs.sort();
    langs.dedup();
    langs
}

/// Plan the depot swap for `language`.
pub(crate) fn plan_language_switch(
    rows: &Obj<'_>,
    app_id: u32,
    installed: &HashMap<u64, u64>,
    manifests: &HashMap<u64, u64>,
    language: &str,
) -> Result<LanguagePlan> {
    let platform = installed_platform(rows, installed);
    let languages = depot_languages(rows, app_id, platform);
    let language = language.to_ascii_lowercase();
    // English often ships in base depots.
    if !languages.is_empty() && !languages.contains(&language) && language != "english" {
        bail!(
            "app {app_id} has no {language} depots; available: english, {}",
            languages.join(", ")
        );
    }

    let on_platform = |row: &Value<'_>| {
        let oslist = row
            .get_obj(&["config"])
            .and_then(|c| c.get("oslist"))
            .and_then(|o| o.as_str());
        should_keep_depot(oslist, platform)
    };
    let owner = |row: &Value<'_>| depots::dlc_app(row, app_id).unwrap_or(app_id);
    // Base game plus installed DLC.
    let mut owners: HashSet<u32> = installed
        .keys()
        .filter_map(|depot| rows.get(&depot.to_string()))
        .map(owner)
        .collect();
    owners.insert(app_id);
    // Owners with target or untagged depots.
    let mut has_target = HashSet::new();
    let mut has_base = HashSet::new();
    let depot_rows = rows
        .iter()
        .filter(|(key, row)| key.parse::<u32>().is_ok() && on_platform(row));
    for (_, row) in depot_rows {
        match depots::depot_language(row) {
            Some(l) if l.eq_ignore_ascii_case(&language) => has_target.insert(owner(row)),
            Some(_) => false,
            None => has_base.insert(owner(row)),
        };
    }

    let mut plan = LanguagePlan::default();
    for (&depot, &manifest) in installed {
        // Drop only replaceable language depots.
        let drop = rows.get(&depot.to_string()).is_some_and(|row| {
            let id = owner(row);
            on_platform(row)
                && depots::depot_language(row).is_some_and(|l| !l.eq_ignore_ascii_case(&language))
                && (has_target.contains(&id) || has_base.contains(&id))
        });
        if drop {
            plan.remove.push((depot as u32, manifest));
        } else {
            plan.keep.push((depot as u32, manifest));
        }
    }
    for (key, row) in rows.iter() {
        let Ok(depot) = key.parse::<u64>() else {
            continue;
        };
        let wanted = on_platform(row)
            && owners.contains(&owner(row))
            && depots::depot_language(row).is_some_and(|l| l.eq_ignore_ascii_case(&language));
        if installed.contains_key(&depot) || !wanted {
            continue;
        }
        if let Some(&manifest) = manifests.get(&depot) {
            plan.add.push((depot as u32, manifest));
            if let Some(dlc) = depots::dlc_app(row, app_id) {
                plan.dlc.insert(depot as u32, dlc);
            }
        }
    }
    plan.remove.sort_unstable();
    plan.add.sort_unstable();
    plan.keep.sort_unstable();
    Ok(plan)
}

/// Files only the removed depots own.
pub(crate) fn files_to_delete(removed: &[Vec<String>], remaining: &[Vec<String>]) -> Vec<String> {
    let keep: HashSet<String> = remaining
        .iter()
        .flatten()
        .map(|f| f.to_ascii_lowercase())
        .collect();
    let mut out: Vec<String> = removed
        .iter()
        .flatten()
        .filter(|f| !keep.contains(&f.to_ascii_lowercase()))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Relative manifest path, safely under `root`.
fn confined_path(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel);
    let safe = rel
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    safe.then(|| root.join(rel))
}

/// Delete files, then emptied folders.
fn delete_files(root: &Path, files: &[String]) -> usize {
    let mut removed = 0;
    let mut dirs = HashSet::new();
    for rel in files {
        let Some(path) = confined_path(root, rel) else {
            tracing::warn!("skipping unsafe depot path {rel:?}");
            continue;
        };
        if path.is_file() && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
        let mut parent = path.parent();
        while let Some(dir) = parent.filter(|d| d.starts_with(root) && *d != root) {
            dirs.insert(dir.to_path_buf());
            parent = dir.parent();
        }
    }
    // Deepest first; non-empty ones fail.
    let mut dirs: Vec<_> = dirs.into_iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for dir in dirs {
        let _ = std::fs::remove_dir(dir);
    }
    removed
}

impl SteamClient {
    /// Switch an installed game's language.
    ///
    /// Download new, delete old-only files.
    pub async fn set_game_language(
        &self,
        appid: u32,
        language: &str,
        shared_state: Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
    ) -> Result<Receiver<DownloadProgress>> {
        let connection = self.require_connection_owned()?;
        let manifest_path = self.appmanifest_path(appid).await?;
        let raw = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("app {appid} is not installed"))?;
        let installed = acf::installed_depots(&raw);
        if installed.is_empty() {
            bail!("app {appid} has no installed depots; install it first");
        }
        let branch = acf::parse_app_manifest(&raw).active_branch;
        let install_root = self.install_root_for_app(appid).await?;
        let language = language.trim().to_ascii_lowercase();
        if language.is_empty() {
            bail!("language must not be empty");
        }

        let resolved = depots::fetch(&connection, appid, None).await?;
        let manifests = resolved.manifest_ids(&branch);
        let plan = plan_language_switch(&resolved.rows, appid, &installed, &manifests, &language)?;
        let game_name = acf::parse_app_manifest(&raw)
            .display_name()
            .unwrap_or_else(|| format!("App {appid}"));

        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let client = self.clone();
        tokio::task::spawn(async move {
            if let Ok(mut state) = shared_state.write() {
                state.begin(
                    appid,
                    game_name.clone(),
                    0,
                    format!("Switching {game_name} to {language}..."),
                );
            }
            emit_queued(&tx, "reading depot file lists").await;

            // Fail before changing anything.
            let mut removed_files = Vec::new();
            let mut remaining_files = Vec::new();
            for (depot, manifest) in &plan.remove {
                match client
                    .fetch_manifest_files(resolved.source_app(*depot), *depot, *manifest)
                    .await
                {
                    Ok(files) => {
                        removed_files.push(files.into_iter().map(|f| f.filename).collect())
                    }
                    Err(e) => {
                        emit_failed_err(&tx, &format!("file list of depot {depot}"), &e).await;
                        return;
                    }
                }
            }
            if !plan.remove.is_empty() {
                for (depot, manifest) in plan.keep.iter().chain(&plan.add) {
                    match client
                        .fetch_manifest_files(resolved.source_app(*depot), *depot, *manifest)
                        .await
                    {
                        Ok(files) => {
                            remaining_files.push(files.into_iter().map(|f| f.filename).collect())
                        }
                        Err(e) => {
                            emit_failed_err(&tx, &format!("file list of depot {depot}"), &e).await;
                            return;
                        }
                    }
                }
            }

            let mut added = Vec::new();
            if !plan.add.is_empty() {
                let selections = plan
                    .add
                    .iter()
                    .map(|&(depot_id, manifest_id)| ManifestSelection {
                        app_id: resolved.source_app(depot_id),
                        depot_id,
                        manifest_id,
                        appinfo_vdf: String::new(),
                        required: true,
                    })
                    .collect();
                let Some(hosts) = fetch_content_hosts(&client, &connection, &tx, appid).await
                else {
                    return;
                };
                spawn_progress_reporter(
                    tx.clone(),
                    shared_state.clone(),
                    DownloadProgressState::Downloading,
                );
                let Some(done) = run_depot_loop(
                    &DepotLoopCtx {
                        client: &client,
                        connection: &connection,
                        tx: &tx,
                        shared_state: &shared_state,
                    },
                    appid,
                    selections,
                    &install_root,
                    &hosts,
                    &DepotLoopOpts {
                        verify_mode: false,
                        grand_total_bytes: 0,
                        manifest_overrides: None,
                    },
                )
                .await
                else {
                    if let Ok(mut state) = shared_state.write() {
                        state.finish("Language switch failed");
                    }
                    return;
                };
                added = done;
            }

            let doomed = files_to_delete(&removed_files, &remaining_files);
            let deleted = delete_files(&install_root, &doomed);
            tracing::info!("Removed {deleted} file(s) of the old language for app {appid}");

            let remove: HashSet<u32> = plan.remove.iter().map(|(d, _)| *d).collect();
            let result = std::fs::read_to_string(&manifest_path)
                .with_context(|| format!("reading {}", manifest_path.display()))
                .and_then(|raw| {
                    let updated = super::manifests::switch_language_in_manifest(
                        &raw, &remove, &added, &plan.dlc, &language,
                    );
                    crate::core::atomic::write_atomic(&manifest_path, updated.as_bytes())
                });
            if let Err(err) = result {
                emit_failed_err(&tx, "appmanifest", &err).await;
                return;
            }
            if let Ok(mut state) = shared_state.write() {
                state.finish("Language switched");
            }
            emit_completed(&tx, "language switched").await;
        });
        Ok(rx)
    }
}

#[cfg(test)]
mod tests;
