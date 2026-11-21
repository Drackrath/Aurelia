use super::*;

#[test]
fn human_bytes_scales_units() {
    assert_eq!(human_bytes(0), "0 B");
    assert_eq!(human_bytes(512), "512 B");
    assert_eq!(human_bytes(1024), "1.00 KiB");
    assert_eq!(human_bytes(1536), "1.50 KiB");
    assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.00 GiB");
}

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
fn rate_tracker_averages_bursts_over_the_window() {
    use std::time::{Duration, Instant};
    let mut r = RateTracker::new();
    let t0 = Instant::now();
    // 10 MB/s, but delivered in 2 s bursts.
    let mut speeds = Vec::new();
    for tick in 0..=80u64 {
        let at = t0 + Duration::from_millis(tick * 250);
        let bytes = (tick / 8) * 20_000_000;
        speeds.push(r.sample_at(at, bytes, 10_000_000_000).0);
    }
    for s in &speeds[24..] {
        assert!((6e6..=14e6).contains(s), "speed {s} outside 6-14 MB/s");
    }
}

#[test]
fn rate_tracker_restarts_when_counter_drops() {
    use std::time::{Duration, Instant};
    let mut r = RateTracker::new();
    let t0 = Instant::now();
    r.sample_at(t0, 5_000_000, 100_000_000);
    r.sample_at(t0 + Duration::from_secs(1), 6_000_000, 100_000_000);
    // A retried depot resets the counters.
    let (s, _) = r.sample_at(t0 + Duration::from_secs(2), 1_000_000, 100_000_000);
    assert!(s >= 0.0);
    let (s, _) = r.sample_at(t0 + Duration::from_secs(3), 3_000_000, 100_000_000);
    assert!((1.9e6..=2.1e6).contains(&s), "{s}");
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
