//! Depot download loop and progress events.

use super::*;

/// Returns whether the user has signalled an abort for the in-progress
/// download/verify. A poisoned lock is treated as "not aborted" so a transient
/// lock failure can't spuriously cancel the operation.
pub(crate) fn download_aborted(
    state: &Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
) -> bool {
    state
        .read()
        .map(|s| s.abort_signal.load(std::sync::atomic::Ordering::Relaxed))
        .unwrap_or(false)
}

/// Send a terminal `Failed` progress message.
pub(crate) async fn emit_failed(
    tx: &tokio::sync::mpsc::Sender<DownloadProgress>,
    msg: impl Into<String>,
) {
    let _ = tx.send(DownloadProgress::failed(msg)).await;
}

/// Send `Failed`, keeping the error's kind.
pub(crate) async fn emit_failed_err(
    tx: &tokio::sync::mpsc::Sender<DownloadProgress>,
    context: &str,
    err: &anyhow::Error,
) {
    let _ = tx.send(DownloadProgress::failed_with(context, err)).await;
}

/// Send `Failed` as a user stop.
pub(crate) async fn emit_cancelled(tx: &tokio::sync::mpsc::Sender<DownloadProgress>) {
    let err = anyhow!(crate::core::error::TypedError::new(
        crate::core::error::ErrorKind::Cancelled,
        "stopped by request",
    ));
    let _ = tx
        .send(DownloadProgress::failed_with("download", &err))
        .await;
}

/// Options for [`run_depot_loop`].
pub(crate) struct DepotLoopOpts {
    /// Re-verify existing chunks instead of downloading.
    pub verify_mode: bool,
    /// Whole-app total; `0` accumulates per-depot.
    pub grand_total_bytes: u64,
    /// Depot-pinned manifests; failures become hard errors.
    pub manifest_overrides: Option<std::collections::HashMap<u32, u64>>,
}

/// Depot fetch loop; `None` reports failure.
/// Content-server hosts; `None` after reporting failure.
pub(crate) async fn fetch_content_hosts(
    client: &SteamClient,
    connection: &Connection,
    tx: &tokio::sync::mpsc::Sender<DownloadProgress>,
    appid: u32,
) -> Option<Vec<ContentHost>> {
    tracing::info!("Fetching Content Servers for AppID: {}...", appid);
    match client.get_content_servers(connection.cell_id()).await {
        Ok(h) => Some(h),
        Err(e) => {
            emit_failed_err(tx, "Failed to fetch content servers", &e).await;
            None
        }
    }
}

pub(crate) async fn run_depot_loop(
    client: &SteamClient,
    connection: &Connection,
    tx: &tokio::sync::mpsc::Sender<DownloadProgress>,
    shared_state: &Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
    appid: u32,
    selections: Vec<ManifestSelection>,
    dest: &Path,
    hosts: &[ContentHost],
    opts: &DepotLoopOpts,
) -> Option<Vec<(u32, u64, u64)>> {
    let verb = if opts.verify_mode {
        "Verifying"
    } else {
        "Downloading"
    };
    let fail_verb = if opts.verify_mode {
        "download/verify"
    } else {
        "download"
    };
    let mut successful_depots = Vec::new();
    let required_selected = selections.iter().any(|selection| selection.required);
    let mut required_successful = 0;
    for selection in selections {
        tracing::info!(
            "Starting download for Depot {} (GID: {})...",
            selection.depot_id,
            selection.manifest_id
        );
        // Reset the current-depot counters so per-depot progress restarts.
        if let Ok(mut state) = shared_state.write() {
            state.status_text = format!("{verb} depot {}", selection.depot_id);
            state.depot_id = selection.depot_id;
            state.depot_downloaded_bytes = 0;
            state.depot_total_bytes = 0;
        }

        // A downgrade names this depot's manifest explicitly, so a request
        // code that comes back 401/empty (typical for very old manifests) is a
        // hard, clearly-reported failure rather than a silent skip.
        let is_override = opts
            .manifest_overrides
            .as_ref()
            .is_some_and(|m| m.contains_key(&selection.depot_id));
        if is_override {
            let ok = matches!(
                client
                    .get_manifest_request_code(selection.app_id, selection.depot_id, selection.manifest_id)
                    .await,
                Ok(code) if code != 0
            );
            if !ok {
                emit_failed(
                    tx,
                    format!(
                        "Steam declined a request code for manifest {} on depot {} — it may be too old, or require owning the game with a non-anonymous login",
                        selection.manifest_id, selection.depot_id
                    ),
                )
                .await;
                return None;
            }
        }

        // Appinfo can advertise mutually exclusive depots from different packages.
        // Only an explicit access denial permits skipping automatically selected content.
        let key = match client
            .get_depot_key(selection.app_id, selection.depot_id)
            .await
        {
            Ok(k) => k,
            Err(e) => {
                if depots::skip_key_failure(&e, is_override, selection.required) {
                    tracing::warn!("Skipping depot {}: {e:#}", selection.depot_id);
                    continue;
                }
                emit_failed(
                    tx,
                    format!(
                        "No depot key for depot {} — installation is incomplete: {}",
                        selection.depot_id, e
                    ),
                )
                .await;
                return None;
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

        match client
            .download_depot_to_with_hosts(
                selection.app_id,
                selection.depot_id,
                selection.manifest_id,
                dest,
                shared_state.clone(),
                hosts,
                opts.grand_total_bytes,
                connection,
                key,
                opts.verify_mode,
            )
            .await
        {
            Ok(depot_size) => {
                if selection.required {
                    required_successful += 1;
                }
                successful_depots.push((selection.depot_id, selection.manifest_id, depot_size));
            }
            Err(e) => {
                if download_aborted(shared_state) {
                    // Else a stop looked like success.
                    emit_cancelled(tx).await;
                    return None;
                }
                let context = format!("Failed to {fail_verb} depot {}", selection.depot_id);
                emit_failed_err(tx, &context, &e).await;
                return None;
            }
        }
    }
    if !depots::has_installable_content(
        successful_depots.len(),
        required_selected,
        required_successful,
    ) {
        emit_failed(
            tx,
            format!("No accessible base-game content installed for app {appid}"),
        )
        .await;
        return None;
    }
    Some(successful_depots)
}

/// Send the initial `Queued` progress message.
pub(crate) async fn emit_queued(
    tx: &tokio::sync::mpsc::Sender<DownloadProgress>,
    msg: impl Into<String>,
) {
    let _ = tx.send(DownloadProgress::queued(msg)).await;
}

/// Send a terminal `Completed` progress message.
pub(crate) async fn emit_completed(
    tx: &tokio::sync::mpsc::Sender<DownloadProgress>,
    msg: impl Into<String>,
) {
    let _ = tx.send(DownloadProgress::completed(msg)).await;
}

/// Forward byte counters every 250ms.
pub(crate) fn spawn_progress_reporter(
    tx: tokio::sync::mpsc::Sender<DownloadProgress>,
    state: Arc<std::sync::RwLock<crate::core::models::DownloadState>>,
    report_state: DownloadProgressState,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(250));
        loop {
            ticker.tick().await;
            let snapshot = match state.read() {
                Ok(s) => Some((
                    s.is_downloading,
                    s.downloaded_bytes,
                    s.reused_bytes,
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
                reused,
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
            if tx
                .send(DownloadProgress {
                    state: report_state,
                    bytes_downloaded: downloaded,
                    reused_bytes: reused,
                    total_bytes: total,
                    current_file: status,
                    depot_id,
                    depot_bytes_downloaded: depot_downloaded,
                    depot_total_bytes: depot_total,
                    error_kind: None,
                })
                .await
                .is_err()
            {
                break;
            }
        }
    })
}
