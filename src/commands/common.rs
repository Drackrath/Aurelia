//! Cross-cutting command helpers shared across command modules.

use crate::daemon;

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use anyhow::{bail, Context, Result};
use aurelia::core::config::load_launcher_config;
use aurelia::core::config::load_library_cache;
use aurelia::core::config::load_session;
use aurelia::library::{build_game_library, scan_installed_app_info};
use aurelia::core::models::{DownloadProgress, DownloadProgressState, DownloadState, LibraryGame};
use aurelia::steam_client::{SharedApp, SteamClient};

/// Print a JSON value to stdout (pretty-printed).
pub(crate) fn print_json(value: &serde_json::Value) {
    match serde_json::to_string_pretty(value) {
        Ok(s) => cli_println!("{s}"),
        Err(_) => cli_println!("{{}}"),
    }
}

/// Print a JSON value as a single compact line (for NDJSON streams).
pub(crate) fn print_json_line(value: &serde_json::Value) {
    match serde_json::to_string(value) {
        Ok(s) => cli_println!("{s}"),
        Err(_) => cli_println!("{{}}"),
    }
}

/// Emit an intermediate NDJSON *event* (progress ticks, login challenges) to
/// **stderr**, keeping stdout reserved for the single terminal result object so
/// a driver like Heroic can `JSON.parse` stdout without scanning for the last
/// value.
pub(crate) fn eprint_json_line(value: &serde_json::Value) {
    match serde_json::to_string(value) {
        Ok(s) => cli_eprintln!("{s}"),
        Err(_) => cli_eprintln!("{{}}"),
    }
}

/// Build a client and restore a persisted session if one exists.
///
/// Inside the daemon this returns a cheap client backed by the **single shared
/// connection** (no logon); only a standalone run actually re-authenticates here.
pub(crate) async fn restored_client() -> Result<SteamClient> {
    if daemon::in_daemon() {
        return Ok(daemon::shared_restored_client().await);
    }
    let mut client = SteamClient::new()?;
    let saved = load_session().await.unwrap_or_default();
    if saved.refresh_token.is_some() && saved.account_name.is_some() {
        tracing::info!("Restoring Steam session (connecting to Steam) ...");
        match client.restore_session().await {
            Ok(_) => tracing::info!("Restored Steam session from refresh token"),
            Err(e) => tracing::warn!("Stored refresh token failed ({e:#}); run `aurelia login`"),
        }
    }
    Ok(client)
}

/// Require an authenticated client, erroring out with a helpful message otherwise.
pub(crate) async fn authed_client() -> Result<SteamClient> {
    let client = restored_client().await?;
    if !client.is_authenticated() {
        bail!("not logged in — run `aurelia login` first");
    }
    Ok(client)
}

/// Build the merged owned + installed library.
pub(crate) async fn load_library(client: &mut SteamClient) -> Vec<LibraryGame> {
    let cached = load_library_cache().await.unwrap_or_default();
    let owned = if client.is_authenticated() {
        tracing::info!("Fetching owned games from Steam ...");
        match client.fetch_owned_games().await {
            Ok(games) => {
                tracing::info!("Fetched {} owned games", games.len());
                games
            }
            Err(e) => {
                tracing::warn!("Could not fetch owned games ({e:#}); using cached library");
                cached
            }
        }
    } else if !cached.is_empty() {
        cached
    } else {
        // Not logged in to Aurelia (and nothing cached). The Steam client is
        // almost always already signed in on Linux and keeps the whole library
        // on disk, so fall back to reading its caches. This makes `list` show
        // the full library instead of only locally-installed games.
        aurelia::library::local_library::discover_local_owned_games().await
    };
    let installed = scan_installed_app_info().await.unwrap_or_default();
    build_game_library(owned, installed, client.steam_id()).games
}

/// Merge Family-Shared apps into the library. Apps already present (e.g. installed,
/// or surfaced via another path) are flagged as family-shared if not owned; apps not
/// yet present are added as non-installed family-shared entries.
pub(crate) fn merge_family_shared(games: &mut Vec<LibraryGame>, shared: Vec<SharedApp>) {
    for app in shared {
        if aurelia::library::is_ignored_steam_app(app.app_id, &app.name) {
            continue;
        }
        if let Some(existing) = games.iter_mut().find(|g| g.app_id == app.app_id) {
            if !existing.is_owned {
                existing.is_family_shared = true;
            }
            continue;
        }
        games.push(LibraryGame {
            app_id: app.app_id,
            name: app.name,
            playtime_forever_minutes: None,
            is_installed: false,
            install_path: None,
            local_manifest_ids: Default::default(),
            update_available: false,
            update_queued: false,
            active_branch: "public".to_string(),
            is_owned: false,
            is_family_shared: true,
            online_required: None,
            platform: None,
        });
    }
}

pub(crate) async fn find_game(client: &mut SteamClient, app_id: u32) -> Result<LibraryGame> {
    load_library(client)
        .await
        .into_iter()
        .find(|g| g.app_id == app_id)
        .with_context(|| format!("app {app_id} is not in your library"))
}

/// Confirm a mutating cloud write, honoring `--yes` and `--json`. Returns `Ok(())`
/// to proceed, or an error to abort. In `--json` mode `--yes` is mandatory.
pub(crate) fn confirm_cloud_write(action: &str, count: usize, yes: bool, json: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if json {
        bail!("refusing to {action} without confirmation — pass `--yes` in --json mode");
    }
    let answer = prompt_line(&format!(
        "About to {action} {count} collection(s) to your Steam account. Continue? [y/N] "
    ))?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        bail!("aborted");
    }
}

/// Human label for a raw EPersonaState value.
pub(crate) fn persona_state_label(state: Option<u32>) -> &'static str {
    match state {
        Some(0) | None => "offline",
        Some(1) => "online",
        Some(2) => "busy",
        Some(3) => "away",
        Some(4) => "snooze",
        Some(5) => "looking to trade",
        Some(6) => "looking to play",
        Some(_) => "online",
    }
}

pub(crate) fn yesno(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// Tracks in-flight installs in this process (the daemon) so `install stop` /
/// `install list` can reach a running download's shared state across the
/// separate forwarded connections that the daemon serves.
pub(crate) static ACTIVE_INSTALLS: OnceLock<Mutex<HashMap<u32, Arc<RwLock<DownloadState>>>>> = OnceLock::new();

pub(crate) fn active_installs() -> &'static Mutex<HashMap<u32, Arc<RwLock<DownloadState>>>> {
    ACTIVE_INSTALLS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// RAII handle: registers an install's shared state on construction and removes
/// it on drop, so the registry entry is cleared on success, error, or `?`/abort.
pub(crate) struct InstallGuard {
    app_id: u32,
}

impl InstallGuard {
    pub(crate) fn register(app_id: u32, state: Arc<RwLock<DownloadState>>) -> Self {
        if let Ok(mut map) = active_installs().lock() {
            map.insert(app_id, state);
        }
        Self { app_id }
    }
}

impl Drop for InstallGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = active_installs().lock() {
            map.remove(&self.app_id);
        }
    }
}

/// Free bytes on the filesystem/drive that `path` lives on, or `None` if it
/// can't be determined (no mounted disk contains the path).
pub(crate) fn available_space_for(path: &std::path::Path) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .iter()
        .filter(|disk| path.starts_with(disk.mount_point()))
        // Longest matching mount point wins (a nested mount over its parent).
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| disk.available_space())
}

/// Format a byte count as a human-readable size (binary units).
pub(crate) fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// Steam edits to appmanifests / `libraryfolders.vdf` are clobbered if Steam is
/// running (it rewrites them on exit). If Steam is up, either stop it (when
/// `restart_steam`) — returning `true` so the caller restarts it afterward — or
/// refuse. Returns whether Steam was stopped.
pub(crate) fn steam_guard_stop(restart_steam: bool, json: bool) -> Result<bool> {
    let running = SteamClient::steam_is_running();
    if running && !restart_steam {
        bail!(
            "Steam is running. Close it first, or re-run with --restart-steam to have \
             Aurelia stop and restart it around the change."
        );
    }
    if running {
        if !json {
            cli_println!("Stopping Steam ...");
        }
        SteamClient::shutdown_steam()?;
        return Ok(true);
    }
    Ok(false)
}

/// Restart Steam if [`steam_guard_stop`] stopped it.
pub(crate) fn steam_guard_restart(managed: bool, json: bool) -> Result<()> {
    if managed {
        if !json {
            cli_println!("Starting Steam ...");
        }
        SteamClient::start_steam()?;
    }
    Ok(())
}

/// Print the final result of a streaming operation (install/verify/update).
pub(crate) fn report_operation(app_id: u32, status: &str, json: bool) {
    if json {
        print_json(&serde_json::json!({ "app_id": app_id, "status": status }));
    }
}

/// Resolve the Steam API language name (e.g. "german", "schinese") to use for
/// user-facing store text: an explicit `--lang` flag wins, else the
/// `aurelia config language` setting, else "english".
pub(crate) async fn resolve_steam_language(flag: Option<String>) -> String {
    match flag {
        Some(l) => l,
        None => load_launcher_config()
            .await
            .ok()
            .and_then(|c| c.language)
            .unwrap_or_else(|| "english".to_string()),
    }
}

/// Best-effort game-name lookup from the offline library cache, for pretty-printing
/// in the `scripts` commands. Returns `None` when the id isn't in the cache.
pub(crate) async fn resolve_game_name(app_id: u32) -> Option<String> {
    load_library_cache()
        .await
        .ok()
        .and_then(|games| games.into_iter().find(|g| g.app_id == app_id).map(|g| g.name))
}

/// Format a Unix timestamp (seconds) as `YYYY-MM-DD HH:MM:SS` (UTC).
pub(crate) fn format_unix_timestamp(secs: u64) -> String {
    let tod = secs % 86_400;
    let (h, m, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    format!(
        "{} {:02}:{:02}:{:02}",
        aurelia::steam_client::unix_to_ymd(secs as i64),
        h,
        m,
        s
    )
}

/// Consume a download/verify progress stream, rendering it to the terminal.
/// In JSON mode each update is emitted as a compact NDJSON line (one object per
/// line) on stdout; the caller still prints the final result object afterward.
pub(crate) async fn drive_progress(
    mut rx: tokio::sync::mpsc::Receiver<DownloadProgress>,
    json: bool,
) -> Result<()> {
    // De-duplicate identical consecutive JSON events (the reporter ticks on a timer,
    // so it would otherwise repeat e.g. "0/0" lines while a manifest is being fetched).
    let mut last: Option<(u8, u64, u64)> = None;
    let mut rate = RateTracker::new();
    while let Some(p) = rx.recv().await {
        match p.state {
            DownloadProgressState::Queued => {
                rate.reset();
                if json {
                    emit_progress_json("queued", &p, 0.0, None, &mut last);
                } else {
                    cli_println!("Queued ...");
                }
            }
            DownloadProgressState::Downloading
            | DownloadProgressState::Verifying
            | DownloadProgressState::Moving => {
                let (state, label) = match p.state {
                    DownloadProgressState::Verifying => ("verifying", "Verifying"),
                    DownloadProgressState::Moving => ("moving", "Moving"),
                    _ => ("downloading", "Downloading"),
                };
                let (speed, eta) = rate.sample(p.bytes_downloaded, p.total_bytes);
                if json {
                    emit_progress_json(state, &p, speed, eta, &mut last);
                } else {
                    print_progress(label, &p, speed, eta);
                }
            }
            DownloadProgressState::Completed => {
                // The caller emits the terminal result object; nothing more here.
                if !json {
                    cli_println!("\nDone.");
                }
                return Ok(());
            }
            DownloadProgressState::Failed => {
                if !json {
                    cli_println!();
                }
                bail!("operation failed: {}", p.current_file);
            }
        }
    }
    Ok(())
}

/// Percentage (one decimal) of `done` out of `total`, 0 when total is unknown.
pub(crate) fn percent_of(done: u64, total: u64) -> f64 {
    if total > 0 {
        ((done as f64 / total as f64) * 1000.0).round() / 10.0
    } else {
        0.0
    }
}

/// Emit one compact NDJSON progress event, skipping it if identical to the last.
/// Reports the whole-app progress (`percent`), the current depot's progress
/// (`depot_percent`), and the transfer rate (`speed_bps`, bytes/sec) and
/// `eta_seconds` (null when not yet estimable).
pub(crate) fn emit_progress_json(
    state: &str,
    p: &DownloadProgress,
    speed_bps: f64,
    eta_seconds: Option<u64>,
    last: &mut Option<(u8, u64, u64)>,
) {
    // Cheap discriminator for the state so we can dedupe (state, overall, depot).
    let state_key = match state {
        "queued" => 0u8,
        "downloading" => 1,
        "verifying" => 2,
        "moving" => 3,
        _ => 4,
    };
    let key = (state_key, p.bytes_downloaded, p.depot_bytes_downloaded);
    if *last == Some(key) {
        return;
    }
    *last = Some(key);

    let value = serde_json::json!({
        "event": "progress",
        "state": state,
        // Whole-app (all depots) progress.
        "bytes_downloaded": p.bytes_downloaded,
        "total_bytes": p.total_bytes,
        "percent": percent_of(p.bytes_downloaded, p.total_bytes),
        // Current depot progress.
        "depot_id": p.depot_id,
        "depot_bytes_downloaded": p.depot_bytes_downloaded,
        "depot_total_bytes": p.depot_total_bytes,
        "depot_percent": percent_of(p.depot_bytes_downloaded, p.depot_total_bytes),
        // Rate / time remaining (for a download-manager progress bar).
        "speed_bps": speed_bps.round() as u64,
        "eta_seconds": eta_seconds,
        "file": p.current_file,
    });
    // Compact single line of NDJSON on stderr, so stdout keeps only the
    // terminal result object.
    if let Ok(s) = serde_json::to_string(&value) {
        cli_eprintln!("{s}");
    }
}

pub(crate) fn print_progress(phase: &str, p: &DownloadProgress, speed_bps: f64, eta_seconds: Option<u64>) {
    let overall = percent_of(p.bytes_downloaded, p.total_bytes);
    let rate = format_rate(speed_bps, eta_seconds);
    if p.depot_id != 0 {
        let depot = percent_of(p.depot_bytes_downloaded, p.depot_total_bytes);
        cli_print!(
            "\r{phase}: {overall:5.1}% overall  {}/{} bytes  | depot {}: {depot:5.1}%{rate}   ",
            p.bytes_downloaded, p.total_bytes, p.depot_id
        );
    } else {
        cli_print!(
            "\r{phase}: {overall:5.1}%  {}/{} bytes{rate}  {}   ",
            p.bytes_downloaded, p.total_bytes, p.current_file
        );
    }
    let _ = std::io::stdout().flush();
}

/// Tracks the transfer rate across successive progress samples, deriving a lightly
/// smoothed speed (bytes/sec) and an ETA. Used by `drive_progress` so every
/// long-running op (download/verify/move) reports speed and time remaining.
pub(crate) struct RateTracker {
    last: Option<(std::time::Instant, u64)>,
    speed_bps: f64,
}

impl RateTracker {
    pub(crate) fn new() -> Self {
        Self { last: None, speed_bps: 0.0 }
    }

    pub(crate) fn reset(&mut self) {
        self.last = None;
        self.speed_bps = 0.0;
    }

    /// Feed the latest cumulative `bytes` (out of `total`); returns
    /// `(speed_bps, eta_seconds)`. Samples closer together than 100 ms are folded
    /// into the next interval to keep the estimate stable.
    pub(crate) fn sample(&mut self, bytes: u64, total: u64) -> (f64, Option<u64>) {
        let now = std::time::Instant::now();
        match self.last {
            Some((t0, b0)) => {
                let dt = now.duration_since(t0).as_secs_f64();
                if dt >= 0.10 && bytes >= b0 {
                    let inst = (bytes - b0) as f64 / dt;
                    // Exponential moving average to damp jitter.
                    self.speed_bps = if self.speed_bps <= 0.0 {
                        inst
                    } else {
                        0.6 * self.speed_bps + 0.4 * inst
                    };
                    self.last = Some((now, bytes));
                }
            }
            None => self.last = Some((now, bytes)),
        }
        let eta = if self.speed_bps > 1.0 && total > bytes {
            Some(((total - bytes) as f64 / self.speed_bps).round() as u64)
        } else {
            None
        };
        (self.speed_bps, eta)
    }
}

/// Human-readable ` 12.34 MiB/s  ETA 00:01:23` suffix (empty when no rate yet).
pub(crate) fn format_rate(speed_bps: f64, eta_seconds: Option<u64>) -> String {
    if speed_bps <= 0.0 {
        return String::new();
    }
    let mib = speed_bps / (1024.0 * 1024.0);
    match eta_seconds {
        Some(s) => format!("  {mib:6.2} MiB/s  ETA {}", format_eta(s)),
        None => format!("  {mib:6.2} MiB/s"),
    }
}

/// Format a seconds count as `HH:MM:SS`.
pub(crate) fn format_eta(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// Word-wrap text to a maximum line width, preserving existing line breaks.
pub(crate) fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
            } else if current.chars().count() + 1 + word.chars().count() <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(std::mem::take(&mut current));
                current.push_str(word);
            }
        }
        lines.push(current);
    }
    lines
}

pub(crate) fn prompt_line(prompt: &str) -> Result<String> {
    // Write the prompt to stderr so stdout stays clean (important for --json).
    cli_eprint!("{prompt}");
    std::io::stderr().flush().ok();
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("failed reading input")?;
    Ok(input.trim().to_string())
}
