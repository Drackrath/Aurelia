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

/// Convert a Unix timestamp (seconds, UTC) to a `YYYY-MM-DD` date string using
/// Howard Hinnant's days-from-civil algorithm. Avoids pulling in a date crate.
pub fn unix_to_ymd(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    // Shift epoch to 0000-03-01 so leap days fall at the end of the era.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests;
