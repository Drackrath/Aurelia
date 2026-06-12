//! Download / verify / move progress rendering and transfer-rate tracking.
//!
//! Consumes the [`DownloadProgress`] stream produced by the content pipeline and
//! renders it either as a live terminal progress bar or as NDJSON `progress`
//! events (`--json`). Split out of `main.rs` so the rate-ticking and formatting
//! logic lives in one place. Routes terminal output through the `cli_*!` macros
//! (`#[macro_use] mod output`) so it works both standalone and via the daemon.

use std::io::Write;

use anyhow::{bail, Result};

use aurelia::models::{DownloadProgress, DownloadProgressState};

/// Consume a download/verify progress stream, rendering it to the terminal.
pub(crate) async fn drive_progress(
    mut rx: tokio::sync::mpsc::Receiver<DownloadProgress>,
    json: bool,
) -> Result<()> {
    // De-duplicate identical consecutive JSON events
    let mut last: Option<(u8, u64, u64)> = None;
    let mut rate = RateTracker::new();
    while let Some(p) = rx.recv().await {
        match p.state {
            DownloadProgressState::Queued => {
                rate.reset();
                if json {
                    emit_progress_json("queued", &p, 0.0, None, &mut last);
                } else {
                    cli_println!("Queued...");
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
fn percent_of(done: u64, total: u64) -> f64 {
    if total > 0 {
        ((done as f64 / total as f64) * 1000.0).round() / 10.0
    } else {
        0.0
    }
}

// Reports the progress (`percent`), (`depot_percent`),
// the transfer rate (`speed_bps`, bytes/sec) `eta_seconds` or null.
fn emit_progress_json(
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
    // Compact single line to NDJSON.
    if let Ok(s) = serde_json::to_string(&value) {
        cli_println!("{s}");
    }
}

fn print_progress(phase: &str, p: &DownloadProgress, speed_bps: f64, eta_seconds: Option<u64>) {
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

/// Tracks the transfer rate across successive progress samples.
struct RateTracker {
    last: Option<(std::time::Instant, u64)>,
    speed_bps: f64,
}

impl RateTracker {
    fn new() -> Self {
        Self { last: None, speed_bps: 0.0 }
    }

    fn reset(&mut self) {
        self.last = None;
        self.speed_bps = 0.0;
    }

    // Feed the latest cumulative `bytes` (out of `total`);
    // returns `(speed_bps, eta_seconds)`.
    fn sample(&mut self, bytes: u64, total: u64) -> (f64, Option<u64>) {
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
fn format_rate(speed_bps: f64, eta_seconds: Option<u64>) -> String {
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
fn format_eta(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eta_formats_hms() {
        assert_eq!(format_eta(0), "00:00:00");
        assert_eq!(format_eta(83), "00:01:23");
        assert_eq!(format_eta(3661), "01:01:01");
    }

    #[test]
    fn rate_tracker_estimates_speed_and_eta() {
        let mut r = RateTracker::new();
        // First sample only primes the tracker (no prior point).
        let (s0, e0) = r.sample(0, 1000);
        assert_eq!(s0, 0.0);
        assert!(e0.is_none());
        // Force a measurable interval, then feed more bytes.
        std::thread::sleep(std::time::Duration::from_millis(120));
        let (s1, e1) = r.sample(200, 1000);
        assert!(s1 > 0.0, "speed should be positive after progress");
        assert!(e1.is_some(), "eta should be estimable once moving");
    }

    #[test]
    fn rate_tracker_resets() {
        let mut r = RateTracker::new();
        let _ = r.sample(100, 1000);
        r.reset();
        let (s, e) = r.sample(0, 1000);
        assert_eq!(s, 0.0);
        assert!(e.is_none());
    }
}
