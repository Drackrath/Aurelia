//! `SteamClient` methods: update-branch, uninstall, move/relink/import, availability.
//!
//! Split out of `steam_client.rs` for readability; the struct, shared imports
//! and free helpers live in the parent module (in scope via `use super::*`).
use super::*;

impl SteamClient {
    /// Resolve an installed app's on-disk layout from its `appmanifest`:
    /// `(manifest, steamapps_dir, library_root, installdir)`. Shared by the
    /// move/relink flows, which differ only in the message reported when no
    /// manifest exists (`missing_manifest_msg`, already formatted by the caller).
    async fn resolve_source_layout(
        &self,
        appid: u32,
        missing_manifest_msg: &str,
    ) -> Result<(PathBuf, PathBuf, PathBuf, String)> {
        let src_manifest = self.appmanifest_path(appid).await?;
        if !src_manifest.exists() {
            bail!("{missing_manifest_msg}");
        }
        let src_steamapps = src_manifest
            .parent()
            .ok_or_else(|| anyhow!("invalid manifest path for app {appid}"))?
            .to_path_buf();
        let src_lib_root = src_steamapps
            .parent()
            .ok_or_else(|| anyhow!("invalid library path for app {appid}"))?
            .to_path_buf();

        let raw = std::fs::read_to_string(&src_manifest)
            .with_context(|| format!("failed reading {}", src_manifest.display()))?;
        let installdir = parse_installdir_from_acf(&raw)
            .ok_or_else(|| anyhow!("appmanifest for {appid} has no installdir"))?;

        Ok((src_manifest, src_steamapps, src_lib_root, installdir))
    }

    pub async fn update_app_branch(&self, appid: u32, branch: &str) -> Result<()> {
        let manifest_path = self.appmanifest_path(appid).await?;
        if !manifest_path.exists() {
            bail!("appmanifest not found for app {appid}");
        }

        let raw = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed reading {}", manifest_path.display()))?;

        let rewritten = rewrite_app_branch(&raw, branch);
        std::fs::write(&manifest_path, rewritten)
            .with_context(|| format!("failed writing {}", manifest_path.display()))?;

        Ok(())
    }

    pub async fn uninstall_game(&self, appid: u32, delete_prefix: bool) -> Result<()> {
        // Resolve the appmanifest across *every* Steam library (including other
        // drives), not just the configured main library. Otherwise uninstalling
        // a game installed on a secondary-drive library fails with "no
        // appmanifest or install directory found", since its manifest lives in
        // that library's steamapps — mirrors how move/relink locate the source.
        let appmanifest = self.appmanifest_path(appid).await?;
        let steamapps = appmanifest
            .parent()
            .ok_or_else(|| anyhow!("invalid steamapps path for app {appid}"))?
            .to_path_buf();

        let installdir = if appmanifest.exists() {
            let raw = std::fs::read_to_string(&appmanifest)
                .with_context(|| format!("failed reading {}", appmanifest.display()))?;
            parse_installdir_from_acf(&raw)
        } else {
            None
        };
        let install_dir = steamapps
            .join("common")
            .join(installdir.unwrap_or_else(|| appid.to_string()));

        // Nothing to remove for an app that was never installed here — report it
        // rather than silently claiming success.
        if !appmanifest.exists() && !install_dir.exists() {
            bail!("app {appid} is not installed (no appmanifest or install directory found)");
        }

        if install_dir.exists() {
            std::fs::remove_dir_all(&install_dir)
                .with_context(|| format!("failed deleting {}", install_dir.display()))?;
        }

        if appmanifest.exists() {
            std::fs::remove_file(&appmanifest)
                .with_context(|| format!("failed deleting {}", appmanifest.display()))?;
        }

        if delete_prefix {
            let compat = steamapps.join("compatdata").join(appid.to_string());
            if compat.exists() {
                std::fs::remove_dir_all(&compat)
                    .with_context(|| format!("failed deleting {}", compat.display()))?;
            }
        }

        Ok(())
    }

    /// Move an installed game to a different Steam library folder.
    ///
    /// Relocates the game files (`steamapps/common/<installdir>`), the Proton
    /// prefix (`steamapps/compatdata/<appid>`, if present) and the
    /// `appmanifest_<appid>.acf`, then updates `libraryfolders.vdf`'s `apps` index
    /// so the Steam client recognises the game at its new path instead of
    /// reporting it as missing. Returns a progress stream (`Moving` events).
    ///
    /// Steam should not be running during the move — it overwrites these files on
    /// exit. The source is only deleted after a successful copy, so an interrupted
    /// move never loses the original install.
    pub async fn move_install(
        &self,
        appid: u32,
        dest_library: PathBuf,
    ) -> Result<Receiver<DownloadProgress>> {
        use crate::relocate;

        // --- Resolve the source layout from the appmanifest ---
        let (src_manifest, src_steamapps, src_lib_root, installdir) = self
            .resolve_source_layout(
                appid,
                &format!("app {appid} is not installed (no appmanifest found)"),
            )
            .await?;

        let src_common = src_steamapps.join("common").join(&installdir);
        if !src_common.exists() {
            bail!("install directory not found: {}", src_common.display());
        }

        // --- Resolve and validate the destination library ---
        let dest_steamapps = dest_library.join("steamapps");
        if !dest_steamapps.exists() {
            bail!(
                "{} is not a Steam library folder (no steamapps/). Add the drive in \
                 Steam \u{2192} Settings \u{2192} Storage first.",
                dest_library.display()
            );
        }
        if dest_steamapps == src_steamapps {
            bail!("app {appid} is already in {}", dest_library.display());
        }

        let dest_common = dest_steamapps.join("common").join(&installdir);
        if dest_common.exists() {
            bail!("destination already exists: {}", dest_common.display());
        }
        let dest_manifest = dest_steamapps.join(format!("appmanifest_{appid}.acf"));

        // Proton prefix, if this game has one.
        let src_compat = src_steamapps.join("compatdata").join(appid.to_string());
        let src_compat = src_compat.exists().then_some(src_compat);
        let dest_compat = src_compat
            .as_ref()
            .map(|_| dest_steamapps.join("compatdata").join(appid.to_string()));

        // Locate the (single) libraryfolders.vdf and warn if the destination isn't
        // a registered library — Steam won't scan an unregistered folder.
        let roots = crate::library::all_library_roots().await;
        if !roots.iter().any(|r| r.join("steamapps") == dest_steamapps) {
            tracing::warn!(
                "{} is not a registered Steam library; Steam may not show the game until \
                 the folder is added in Settings \u{2192} Storage",
                dest_library.display()
            );
        }
        let libraryfolders = relocate::find_libraryfolders_vdf(&roots);

        let (tx, rx) = tokio::sync::mpsc::channel(64);

        tokio::task::spawn_blocking(move || {
            let result = (|| -> Result<()> {
                let _ = tx.blocking_send(DownloadProgress {
                    state: DownloadProgressState::Queued,
                    current_file: "sizing".to_string(),
                    ..Default::default()
                });

                let common_bytes = relocate::dir_size(&src_common);
                let compat_bytes = src_compat.as_ref().map(|p| relocate::dir_size(p)).unwrap_or(0);
                let total = common_bytes + compat_bytes;

                // Both phases emit identical `Moving` events; the prefix phase just
                // offsets its byte count past the already-copied game files.
                let send_moving = |base: u64, copied: u64, file: &str| {
                    let _ = tx.blocking_send(DownloadProgress {
                        state: DownloadProgressState::Moving,
                        bytes_downloaded: base + copied,
                        total_bytes: total,
                        current_file: file.to_string(),
                        ..Default::default()
                    });
                };

                // Game files.
                relocate::move_dir_with_progress(&src_common, &dest_common, common_bytes, |copied, file| {
                    send_moving(0, copied, file);
                })
                .with_context(|| format!("failed moving game files to {}", dest_common.display()))?;

                // Proton prefix.
                if let (Some(sc), Some(dc)) = (&src_compat, &dest_compat) {
                    relocate::move_dir_with_progress(sc, dc, compat_bytes, |copied, file| {
                        send_moving(common_bytes, copied, file);
                    })
                    .with_context(|| format!("failed moving Proton prefix to {}", dc.display()))?;
                }

                // appmanifest: copy to the new library, then remove the original so
                // Steam sees the game in exactly one place.
                std::fs::copy(&src_manifest, &dest_manifest)
                    .with_context(|| format!("failed writing {}", dest_manifest.display()))?;
                std::fs::remove_file(&src_manifest)
                    .with_context(|| format!("failed removing {}", src_manifest.display()))?;

                // libraryfolders.vdf apps index (best-effort; Steam reconciles from
                // the appmanifests on next launch if this can't be edited cleanly).
                if let Some(vdf_path) = &libraryfolders {
                    match std::fs::read_to_string(vdf_path) {
                        Ok(text) => {
                            match relocate::update_libraryfolders_apps(
                                &text, appid, &src_lib_root, &dest_library, common_bytes,
                            ) {
                                Some(updated) => {
                                    if let Err(e) = std::fs::write(vdf_path, updated) {
                                        tracing::warn!(
                                            "moved game but could not write libraryfolders.vdf: {e}"
                                        );
                                    }
                                }
                                None => tracing::warn!(
                                    "could not locate library entries in libraryfolders.vdf; \
                                     Steam will reconcile the index on next launch"
                                ),
                            }
                        }
                        Err(e) => tracing::warn!("could not read libraryfolders.vdf: {e}"),
                    }
                }

                Ok(())
            })();

            match result {
                Ok(()) => {
                    let _ = tx.blocking_send(DownloadProgress {
                        state: DownloadProgressState::Completed,
                        ..Default::default()
                    });
                }
                Err(e) => {
                    let _ = tx.blocking_send(DownloadProgress {
                        state: DownloadProgressState::Failed,
                        current_file: format!("{e:#}"),
                        ..Default::default()
                    });
                }
            }
        });

        Ok(rx)
    }

    /// Whether a game is installed and its files are present on disk
    pub async fn is_game_available(&self, appid: u32) -> (bool, Option<String>) {
        let Ok(manifest) = self.appmanifest_path(appid).await else {
            return (false, None);
        };
        if !manifest.exists() {
            return (false, None);
        }
        // A manifest written at install start
        match std::fs::read_to_string(&manifest) {
            Ok(raw) if !manifest_is_fully_installed(&raw) => return (false, None),
            Ok(_) => {}
            Err(_) => return (false, None),
        }
        match self.install_root_for_app(appid).await {
            Ok(path) => (path.exists(), Some(path.to_string_lossy().into_owned())),
            Err(_) => (false, None),
        }
    }

    /// Relink 
    pub async fn relink_install(&self, appid: u32, dest_library: PathBuf) -> Result<PathBuf> {
        let (src_manifest, src_steamapps, src_lib_root, installdir) = self
            .resolve_source_layout(
                appid,
                &format!("app {appid} is not registered (no appmanifest to relink)"),
            )
            .await?;

        let dest_steamapps = dest_library.join("steamapps");
        if !dest_steamapps.exists() {
            bail!(
                "{} is not a Steam library folder (no steamapps/)",
                dest_library.display()
            );
        }
        if dest_steamapps == src_steamapps {
            bail!("app {appid} is already linked to {}", dest_library.display());
        }
        let dest_common = dest_steamapps.join("common").join(&installdir);
        if !dest_common.exists() {
            bail!(
                "game files not found at {} — relink only updates Steam's records; use \
                 `move` to copy the files there first",
                dest_common.display()
            );
        }
        let dest_manifest = dest_steamapps.join(format!("appmanifest_{appid}.acf"));

        // Move only the manifest (files are already in place).
        std::fs::copy(&src_manifest, &dest_manifest)
            .with_context(|| format!("failed writing {}", dest_manifest.display()))?;
        std::fs::remove_file(&src_manifest)
            .with_context(|| format!("failed removing {}", src_manifest.display()))?;

        update_libraryfolders_for(&src_lib_root, &dest_library, appid, &dest_common).await;
        Ok(dest_common)
    }

    /// Import an on-disk install that Steam doesn't know about: write an
    /// `appmanifest_<appid>.acf` for the existing files in `library` and register
    /// it in `libraryfolders.vdf`. Depot manifests and the build id come from PICS
    /// so Steam sees the game as installed and up to date. Steam should not be
    /// running. Returns the install directory.
    pub async fn import_install(
        &self,
        appid: u32,
        library: PathBuf,
        platform: DepotPlatform,
    ) -> Result<PathBuf> {
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
            .context("failed requesting appinfo product info for import")?;
        let app = response
            .apps
            .iter()
            .find(|e| e.appid() == appid)
            .ok_or_else(|| anyhow!("missing appinfo payload for app {appid}"))?;

        let vdf = find_vdf_in_pics(app.buffer()).context("failed to parse product info VDF")?;
        let root_obj = vdf.as_obj().context("root is not an object")?;
        let app_obj = if vdf.key() == "appinfo" || vdf.key() == appid.to_string() {
            root_obj
        } else {
            root_obj
                .get("appinfo")
                .and_then(|v| v.as_obj())
                .unwrap_or(root_obj)
        };

        let common = app_obj.get("common").and_then(|v| v.as_obj());
        let name = common
            .and_then(|c| c.get("name"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("App {appid}"));
        let installdir = common
            .and_then(|c| c.get("installdir"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("PICS appinfo for {appid} has no installdir; cannot import"))?;

        let depots_obj = app_obj.get("depots").and_then(|v| v.as_obj());
        let buildid = depots_obj
            .and_then(|d| d.get("branches"))
            .and_then(|v| v.as_obj())
            .and_then(|b| b.get("public"))
            .and_then(|v| v.as_obj())
            .and_then(|p| p.get("buildid"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Platform-matched, non-DLC depots with a public manifest → InstalledDepots.
        let mut installed_depots: Vec<(u32, u64, u64)> = Vec::new();
        if let Some(depots) = depots_obj {
            for (key, value) in depots.iter() {
                let Ok(depot_id) = key.parse::<u32>() else {
                    continue;
                };
                let Some(obj) = value.as_obj() else { continue };
                if obj.get("dlcappid").is_some() {
                    continue;
                }
                let oslist = obj
                    .get("config")
                    .and_then(|v| v.as_obj())
                    .and_then(|c| c.get("oslist"))
                    .and_then(|v| v.as_str());
                if !should_keep_depot(oslist, platform) {
                    continue;
                }
                let Some(public) = obj
                    .get("manifests")
                    .and_then(|v| v.as_obj())
                    .and_then(|m| m.get("public"))
                    .and_then(|v| v.as_obj())
                else {
                    continue;
                };
                if let Some(mid) = public
                    .get("gid")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    let size = public
                        .get("size")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0);
                    installed_depots.push((depot_id, mid, size));
                }
            }
        }

        let steamapps = library.join("steamapps");
        if !steamapps.exists() {
            bail!(
                "{} is not a Steam library folder (no steamapps/)",
                library.display()
            );
        }
        let common_dir = steamapps.join("common").join(&installdir);
        if !common_dir.exists() {
            bail!(
                "game files not found at {} — `import` registers existing files; use \
                 `install` to download the game",
                common_dir.display()
            );
        }
        let manifest = steamapps.join(format!("appmanifest_{appid}.acf"));
        if manifest.exists() {
            bail!("app {appid} is already registered at {}", manifest.display());
        }

        Self::write_appmanifest(
            &manifest,
            appid,
            &name,
            &installdir,
            installed_depots,
            buildid.as_deref(),
            true,
        )?;

        // Register in libraryfolders.vdf (add to this library; nothing to remove).
        update_libraryfolders_for(&library, &library, appid, &common_dir).await;
        Ok(common_dir)
    }

}
