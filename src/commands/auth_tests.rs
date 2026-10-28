use super::*;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

// Fake stdin from a line script.
fn lines(script: &[Option<&str>]) -> impl FnMut() -> std::future::Ready<std::io::Result<Option<String>>> {
    let queue: Arc<Mutex<VecDeque<Option<String>>>> = Arc::new(Mutex::new(
        script.iter().map(|l| l.map(str::to_string)).collect(),
    ));
    move || std::future::ready(Ok(queue.lock().unwrap().pop_front().flatten()))
}

#[tokio::test]
async fn cancel_line_cancels() {
    let kind = watch_control(lines(&[Some(""), Some("noise"), Some("  Cancel\r")])).await;
    assert_eq!(kind, ErrorKind::Cancelled);
}

#[tokio::test]
async fn eof_closes_input() {
    assert_eq!(watch_control(lines(&[Some("noise"), None])).await, ErrorKind::InputClosed);
}

#[tokio::test]
async fn read_error_closes_input() {
    let kind = watch_control(|| std::future::ready(Err(std::io::Error::other("gone")))).await;
    assert_eq!(kind, ErrorKind::InputClosed);
}

#[tokio::test]
async fn control_ends_a_pending_login() {
    let work = std::future::pending::<Result<()>>();
    let err = race_control(work, async { ErrorKind::Cancelled }).await.unwrap_err();
    assert_eq!(classify(&err).kind, ErrorKind::Cancelled);
}

#[tokio::test]
async fn finished_login_wins() {
    let out = race_control(async { Ok(7) }, std::future::pending()).await.unwrap();
    assert_eq!(out, 7);
}

#[test]
fn approved_result_line() {
    let v = qr_result_ok("acct", Some(76561197960287930));
    assert_eq!(v["event"], "result");
    assert_eq!(v["ok"], true);
    assert_eq!(v["type"], "approved");
    assert_eq!(v["account"], "acct");
    assert_eq!(v["steam_id"], 76561197960287930u64);
}

#[test]
fn failed_result_line_is_typed() {
    for (kind, name) in [
        (ErrorKind::LoginDenied, "login_denied"),
        (ErrorKind::LoginExpired, "login_expired"),
        (ErrorKind::Timeout, "timeout"),
        (ErrorKind::Cancelled, "cancelled"),
        (ErrorKind::InputClosed, "input_closed"),
    ] {
        let err = anyhow!(TypedError::new(kind, "why")).context("QR login failed");
        let v = qr_result_err(&err);
        assert_eq!(v["event"], "result");
        assert_eq!(v["ok"], false);
        assert_eq!(v["type"], name);
        assert_eq!(v["error"], "QR login failed: why");
        assert_eq!(v.get("hint").is_some(), kind.hint().is_some());
    }
}

#[test]
fn outcome_exit_codes_are_distinct() {
    let codes = [
        0,
        ErrorKind::LoginDenied.exit_code(),
        ErrorKind::LoginExpired.exit_code(),
        ErrorKind::Timeout.exit_code(),
        ErrorKind::Cancelled.exit_code(),
        ErrorKind::InputClosed.exit_code(),
        ErrorKind::Unknown.exit_code(),
    ];
    let unique: std::collections::HashSet<_> = codes.iter().collect();
    assert_eq!(unique.len(), codes.len(), "{codes:?}");
}

#[test]
fn reported_keeps_its_kind() {
    let inner = anyhow!(TypedError::new(ErrorKind::Timeout, "late"));
    let err = anyhow!(Reported(inner));
    assert_eq!(classify(&err).kind, ErrorKind::Timeout);
    assert_eq!(classify(&err).kind.exit_code(), 82);
}
