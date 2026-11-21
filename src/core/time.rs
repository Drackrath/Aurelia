//! Unix time and polling helpers.

use anyhow::Result;

/// Time elapsed since the Unix epoch.
pub fn since_unix_epoch() -> Result<std::time::Duration, std::time::SystemTimeError> {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
}

/// Current Unix time in seconds.
pub fn now_unix() -> u64 {
    since_unix_epoch().map(|d| d.as_secs()).unwrap_or(0)
}

/// Unix seconds of `time`, if post-epoch.
pub fn unix_secs(time: std::time::SystemTime) -> Option<u64> {
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// Polls `done` until true or timeout.
pub fn wait_until(
    mut done: impl FnMut() -> bool,
    timeout: std::time::Duration,
    interval: std::time::Duration,
) -> bool {
    let start = std::time::Instant::now();
    loop {
        if done() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(interval);
    }
}

#[cfg(test)]
mod tests;
