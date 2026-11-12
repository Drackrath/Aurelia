use super::wait_until;
use std::time::Duration;

#[test]
fn done_at_once() {
    let mut polls = 0;
    let ok = wait_until(
        || {
            polls += 1;
            true
        },
        Duration::from_secs(5),
        Duration::from_millis(1),
    );
    assert!(ok);
    assert_eq!(polls, 1);
}

#[test]
fn done_after_n_polls() {
    let mut polls = 0;
    let ok = wait_until(
        || {
            polls += 1;
            polls >= 3
        },
        Duration::from_secs(5),
        Duration::from_millis(1),
    );
    assert!(ok);
    assert_eq!(polls, 3);
}

#[test]
fn times_out() {
    let mut polls = 0;
    let ok = wait_until(
        || {
            polls += 1;
            false
        },
        Duration::from_millis(20),
        Duration::from_millis(5),
    );
    assert!(!ok);
    assert!(polls >= 2);
}
