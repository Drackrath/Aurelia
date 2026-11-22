//! Steam Cloud client and sync runs.

use super::*;

/// Stamp a downloaded save with its cloud mtime so the next `sync_up` doesn't see it
/// as locally modified and re-upload it (and so Steam's own comparison stays stable).
pub(super) fn set_file_mtime_secs(path: &Path, secs: u64) {
    let mtime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    let times = std::fs::FileTimes::new().set_modified(mtime);
    if let Err(e) = std::fs::set_times(path, times) {
        tracing::warn!("could not set mtime on {}: {e}", path.display());
    }
}

pub struct CloudClient {
    pub(super) connection: Connection,
    pub(super) steam_id: u64,
    pub(super) http: reqwest::Client,
}

impl CloudClient {
    pub fn new(mut connection: Connection) -> Self {
        let steam_id = u64::from(connection.steam_id());
        connection.set_timeout(CLOUD_RPC_TIMEOUT);
        Self {
            connection,
            steam_id,
            http: cloud_http_client(),
        }
    }

    pub fn steam_id(&self) -> u64 {
        self.steam_id
    }

    pub async fn get_file_list(&self, appid: u32) -> Result<Vec<CloudFileEntry>> {
        let request = CCloud_EnumerateUserFiles_Request {
            appid: Some(appid),
            extended_details: Some(true),
            ..Default::default()
        };

        let response: CCloud_EnumerateUserFiles_Response = self
            .connection
            .service_method(request)
            .await
            .context("failed calling Cloud.EnumerateUserFiles")?;

        Ok(response
            .files
            .into_iter()
            .map(|file| CloudFileEntry {
                filename: file.filename().to_string(),
                timestamp: file.timestamp(),
                size: u64::from(file.file_size()),
                sha_hash: file.file_sha,
            })
            .collect())
    }

    /// Download cloud saves that are newer than (or missing) their local copy.
    /// Thin wrapper over [`sync`](Self::sync) restricted to the download direction;
    /// `Detect` so a divergent file is reported, never silently overwritten.
    pub async fn sync_down(&self, appid: u32, resolver: &CloudPathResolver) -> Result<SyncOutcome> {
        self.sync(
            appid,
            resolver,
            &[],
            SyncDirection::Down,
            ConflictPolicy::Detect,
        )
        .await
    }

    /// Upload local saves that are new or newer than their cloud copy. Thin wrapper
    /// over [`sync`](Self::sync) restricted to the upload direction.
    pub async fn sync_up(
        &self,
        appid: u32,
        resolver: &CloudPathResolver,
        specs: &[UfsSaveSpec],
    ) -> Result<SyncOutcome> {
        self.sync(
            appid,
            resolver,
            specs,
            SyncDirection::Up,
            ConflictPolicy::Detect,
        )
        .await
    }

    /// Reconcile a game's Steam Cloud saves with their on-disk copies.
    ///
    /// For each save it compares the **content hash** of the cloud and local copies
    /// against a persisted `SyncBaseline` (the last state at which the two agreed):
    ///
    /// - identical content → nothing to do;
    /// - only one side changed since the baseline → move it the obvious way
    ///   (upload a newer local save, download a newer cloud save);
    /// - **both** sides changed independently → a *conflict*: the two copies have
    ///   diverged and picking one by timestamp could throw away real progress.
    ///   `policy` decides what happens — [`ConflictPolicy::Detect`] reports it for a
    ///   Take-Cloud/Take-Local choice without touching either copy, while
    ///   `TakeCloud`/`TakeLocal` apply that choice.
    ///
    /// `direction` filters only the *automatic* transfers (`Down`/`Up`/`Both`);
    /// conflicts are always detected, and an explicit `policy` resolution always
    /// runs regardless of direction.
    pub async fn sync(
        &self,
        appid: u32,
        resolver: &CloudPathResolver,
        specs: &[UfsSaveSpec],
        direction: SyncDirection,
        policy: ConflictPolicy,
    ) -> Result<SyncOutcome> {
        let remote_map: HashMap<String, CloudFileEntry> = self
            .get_file_list(appid)
            .await?
            .into_iter()
            .map(|e| (e.filename.clone(), e))
            .collect();

        // Candidate filenames: every cloud file, plus local saves the UFS rules
        // surface that aren't in the cloud yet (so first-time uploads are included).
        let mut names: BTreeSet<String> = remote_map.keys().cloned().collect();
        let mut local_only: HashMap<String, PathBuf> = HashMap::new();
        for (cloud_name, local_path) in discover_local_saves(specs, resolver) {
            if !remote_map.contains_key(&cloud_name) {
                names.insert(cloud_name.clone());
                local_only.insert(cloud_name, local_path);
            }
        }

        let mut baseline = SyncBaseline::load(appid);
        let mut outcome = SyncOutcome::default();

        for name in names {
            let cloud = remote_map.get(&name);
            let local_path = match local_only.get(&name) {
                Some(p) => p.clone(),
                None => match resolver.resolve(&name) {
                    Ok(p) => p,
                    Err(e) => {
                        // Record it: a caller that only counts transfers would
                        // otherwise report a clean "0 down" for a cloud full of
                        // saves it simply couldn't place.
                        tracing::warn!("skipping cloud file '{name}': {e:#}");
                        outcome.skipped.push(SyncSkip {
                            filename: name.clone(),
                            root_token: split_root_token(&name).map(|(t, _)| t.to_string()),
                            reason: format!("{e:#}"),
                        });
                        continue;
                    }
                },
            };
            let local = read_local_info(&local_path);

            match plan_action(baseline.files.get(&name), local.as_ref(), cloud) {
                PlannedAction::Skip => {
                    // In sync (or nothing to do) — record the agreed fingerprint so
                    // a later one-sided edit is recognised as such.
                    if let (Some(l), Some(_)) = (&local, cloud) {
                        baseline.files.insert(name.clone(), l.as_baseline());
                    }
                }
                PlannedAction::Download => {
                    if direction == SyncDirection::Up {
                        continue; // a pending down-change we're not applying now
                    }
                    let Some(c) = cloud else { continue };
                    self.apply_download(appid, &name, &local_path, c, &mut baseline, &mut outcome)
                        .await;
                }
                PlannedAction::Upload => {
                    if direction == SyncDirection::Down {
                        continue;
                    }
                    let Some(l) = &local else { continue };
                    self.apply_upload(appid, &name, &local_path, l, &mut baseline, &mut outcome)
                        .await;
                }
                PlannedAction::Conflict => {
                    let (Some(l), Some(c)) = (&local, cloud) else {
                        continue;
                    };
                    match policy {
                        ConflictPolicy::Detect => {
                            outcome.conflicts.push(SyncConflict {
                                filename: name.clone(),
                                local_path: local_path.to_string_lossy().into_owned(),
                                local_hash: l.hash.clone(),
                                local_size: l.size,
                                local_timestamp: l.timestamp,
                                cloud_hash: c.sha_hash.clone().unwrap_or_default(),
                                cloud_size: c.size,
                                cloud_timestamp: c.timestamp,
                            });
                        }
                        ConflictPolicy::TakeCloud => {
                            self.apply_download(
                                appid,
                                &name,
                                &local_path,
                                c,
                                &mut baseline,
                                &mut outcome,
                            )
                            .await;
                        }
                        ConflictPolicy::TakeLocal => {
                            self.apply_upload(
                                appid,
                                &name,
                                &local_path,
                                l,
                                &mut baseline,
                                &mut outcome,
                            )
                            .await;
                        }
                    }
                }
            }
        }

        baseline.save(appid);
        Ok(outcome)
    }

    /// Download one cloud file to `local_path` and stamp its mtime to the cloud's,
    /// so the next sync sees the two as in step rather than locally modified.
    /// Download and record baseline/outcome.
    pub(super) async fn apply_download(
        &self,
        appid: u32,
        name: &str,
        local_path: &Path,
        cloud: &CloudFileEntry,
        baseline: &mut SyncBaseline,
        outcome: &mut SyncOutcome,
    ) {
        match self.download_to(appid, name, local_path, cloud).await {
            Ok(()) => {
                baseline
                    .files
                    .insert(name.to_string(), cloud_baseline(cloud, local_path));
                outcome.downloaded.push(name.to_string());
            }
            Err(e) => outcome.record_failure(name, "down", &e),
        }
    }

    /// Upload and record baseline/outcome.
    pub(super) async fn apply_upload(
        &self,
        appid: u32,
        name: &str,
        local_path: &Path,
        local: &LocalInfo,
        baseline: &mut SyncBaseline,
        outcome: &mut SyncOutcome,
    ) {
        match self
            .upload_from(appid, name, local_path, local.timestamp)
            .await
        {
            Ok(()) => {
                baseline.files.insert(name.to_string(), local.as_baseline());
                outcome.uploaded.push(name.to_string());
            }
            Err(e) => outcome.record_failure(name, "up", &e),
        }
    }

    pub(super) async fn download_to(
        &self,
        appid: u32,
        filename: &str,
        local_path: &Path,
        cloud: &CloudFileEntry,
    ) -> Result<()> {
        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("failed to create parent directory {}", parent.display())
            })?;
        }
        let what = format!("cloud download '{filename}'");
        let body = retry_transfer(&what, &TRANSFER_BACKOFF, |attempt| {
            tracing::info!("{what} for app {appid} (attempt {attempt})");
            // Fresh signed URL per attempt.
            self.download_file(appid, filename, cloud.sha_hash.as_deref())
        })
        .await
        .with_context(|| format!("failed downloading cloud file '{filename}' for app {appid}"))?;
        tokio::fs::write(local_path, &body)
            .await
            .with_context(|| format!("failed writing {}", local_path.display()))?;
        set_file_mtime_secs(local_path, cloud.timestamp);
        Ok(())
    }

    pub(super) async fn upload_from(
        &self,
        appid: u32,
        filename: &str,
        local_path: &Path,
        local_timestamp: u64,
    ) -> Result<()> {
        let data = tokio::fs::read(local_path)
            .await
            .with_context(|| format!("failed reading {}", local_path.display()))?;
        self.upload_file(appid, filename, local_timestamp, data)
            .await
            .with_context(|| format!("failed uploading cloud file '{filename}' for app {appid}"))
    }

    pub(super) async fn download_file(
        &self,
        appid: u32,
        filename: &str,
        expected_sha: Option<&str>,
    ) -> Result<Vec<u8>> {
        let request = CCloud_ClientFileDownload_Request {
            appid: Some(appid),
            filename: Some(filename.to_string()),
            ..Default::default()
        };

        let response: CCloud_ClientFileDownload_Response = self
            .connection
            .service_method(request)
            .await
            .context("failed calling Cloud.ClientFileDownload")?;

        let url = cloud_transfer_url(
            response.use_https(),
            response.url_host(),
            response.url_path(),
            "ClientFileDownload",
        )?;
        let headers = build_header_map(response.request_headers.iter().map(|h| {
            (
                h.name.as_deref().unwrap_or_default(),
                h.value.as_deref().unwrap_or_default(),
            )
        }))?;

        let raw_file_size = response.raw_file_size();
        let stored_size = response.file_size();

        let http_response = self
            .http
            .get(url)
            .headers(headers)
            .send()
            .await
            .map_err(http_error)
            .context("failed cloud HTTP GET")?
            .error_for_status()
            .map_err(http_error)
            .context("cloud HTTP GET returned failure status")?;

        let limit = cloud_body_limit(stored_size, raw_file_size);
        let body = read_body_capped(http_response, limit)
            .await
            .context("failed reading cloud download body")?;

        // Steam stores cloud files compressed: the transfer is then a ZIP container
        // holding the single real file. Writing the body straight to disk yields a
        // zip where the game expects its save.
        Ok(maybe_decompress(
            body,
            stored_size,
            raw_file_size,
            expected_sha,
            filename,
        ))
    }

    pub(super) async fn upload_file(
        &self,
        appid: u32,
        filename: &str,
        timestamp: u64,
        data: Vec<u8>,
    ) -> Result<()> {
        let mut sha = Sha1::new();
        sha.update(&data);
        let file_sha = sha.finalize().to_vec();
        let file_size = u32::try_from(data.len()).context("cloud upload larger than u32")?;

        let begin_request = CCloud_ClientBeginFileUpload_Request {
            appid: Some(appid),
            file_size: Some(file_size),
            raw_file_size: Some(file_size),
            file_sha: Some(file_sha.clone()),
            time_stamp: Some(timestamp),
            filename: Some(filename.to_string()),
            ..Default::default()
        };

        let begin_response: CCloud_ClientBeginFileUpload_Response = self
            .connection
            .service_method(begin_request)
            .await
            .context("failed calling Cloud.ClientBeginFileUpload")?;

        let blocks = self
            .upload_blocks(filename, &data, begin_response.block_requests)
            .await;
        // Failed uploads still commit, as failed.
        let succeeded = blocks.is_ok();

        let commit_request = CCloud_ClientCommitFileUpload_Request {
            transfer_succeeded: Some(succeeded),
            appid: Some(appid),
            file_sha: Some(file_sha),
            filename: Some(filename.to_string()),
            ..Default::default()
        };

        let commit: Result<CCloud_ClientCommitFileUpload_Response> = self
            .connection
            .service_method(commit_request)
            .await
            .context("failed calling Cloud.ClientCommitFileUpload");
        blocks?;
        let commit_response = commit?;

        if !commit_response.file_committed() {
            return Err(anyhow!(
                "Cloud.ClientCommitFileUpload returned file_committed=false"
            ));
        }

        Ok(())
    }

    /// Send every upload block, each retried.
    pub(super) async fn upload_blocks(
        &self,
        filename: &str,
        data: &[u8],
        block_requests: Vec<ClientCloudFileUploadBlockDetails>,
    ) -> Result<()> {
        for mut block in block_requests {
            let url = cloud_transfer_url(
                block.use_https(),
                block.url_host(),
                block.url_path(),
                "ClientBeginFileUpload",
            )?;

            let block_offset = usize::try_from(block.block_offset()).unwrap_or(0);
            let block_length = usize::try_from(block.block_length()).unwrap_or(data.len());
            let end = block_offset.saturating_add(block_length).min(data.len());
            let payload = if block.explicit_body_data.is_some() {
                block.take_explicit_body_data()
            } else if block_offset < data.len() {
                data[block_offset..end].to_vec()
            } else {
                data.to_vec()
            };

            let method = match block.http_method() {
                1 => reqwest::Method::PUT,
                2 => reqwest::Method::POST,
                _ => reqwest::Method::PUT,
            };
            let headers = build_header_map(block.request_headers.iter().map(|h| {
                (
                    h.name.as_deref().unwrap_or_default(),
                    h.value.as_deref().unwrap_or_default(),
                )
            }))?;

            let deadline = upload_deadline(payload.len());
            let what = format!("cloud upload block of '{filename}'");
            // Blocks retry on their own URL.
            retry_transfer(&what, &TRANSFER_BACKOFF, |attempt| {
                tracing::info!("{what} ({} bytes, attempt {attempt})", payload.len());
                let request = self
                    .http
                    .request(method.clone(), &url)
                    .headers(headers.clone())
                    .timeout(deadline)
                    .body(payload.clone());
                async move {
                    request
                        .send()
                        .await
                        .map_err(http_error)
                        .context("failed cloud HTTP upload")?
                        .error_for_status()
                        .map_err(http_error)
                        .context("cloud HTTP upload returned failure status")?;
                    Ok(())
                }
            })
            .await?;
        }
        Ok(())
    }
}
