use super::*;

use aurelia::core::error::{ErrorKind, classify};
use aurelia::core::models::DownloadProgress;

async fn drive(messages: Vec<DownloadProgress>) -> Result<()> {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    for m in messages {
        tx.send(m).await.unwrap();
    }
    drop(tx);
    drive_progress(rx, true).await
}

#[tokio::test]
async fn typed_failure_keeps_its_kind() {
    let io = std::io::Error::from(std::io::ErrorKind::StorageFull);
    let failed = DownloadProgress::failed_with("depot 7", &anyhow::Error::new(io));
    let err = drive(vec![failed]).await.unwrap_err();
    assert_eq!(classify(&err).kind, ErrorKind::DiskFull);
    assert_eq!(classify(&err).kind.exit_code(), 85);
    assert!(err.to_string().contains("depot 7"));
}

#[tokio::test]
async fn untyped_failure_stays_unknown() {
    let err = drive(vec![DownloadProgress::failed("No matching depots")])
        .await
        .unwrap_err();
    assert_eq!(classify(&err).kind, ErrorKind::Unknown);
}

#[tokio::test]
async fn completed_is_ok() {
    assert!(drive(vec![DownloadProgress::completed("")]).await.is_ok());
}

fn restart_stub(fail: bool, called: &std::cell::Cell<bool>) -> impl FnOnce() -> Result<()> + '_ {
    move || {
        called.set(true);
        if fail {
            bail!("restart failed")
        }
        Ok(())
    }
}

#[test]
fn settle_op_ok_restart_ok() {
    let called = std::cell::Cell::new(false);
    let out = settle_with_restart(Ok(7), restart_stub(false, &called));
    assert_eq!(out.unwrap(), 7);
    assert!(called.get());
}

#[test]
fn settle_op_err_restart_ok_returns_op_err() {
    let called = std::cell::Cell::new(false);
    let out: Result<()> = settle_with_restart(
        Err(anyhow::anyhow!("op failed")),
        restart_stub(false, &called),
    );
    assert_eq!(out.unwrap_err().to_string(), "op failed");
    assert!(called.get());
}

#[test]
fn settle_op_ok_restart_err_returns_restart_err() {
    let called = std::cell::Cell::new(false);
    let out = settle_with_restart(Ok(1), restart_stub(true, &called));
    assert_eq!(out.unwrap_err().to_string(), "restart failed");
}

#[test]
fn settle_both_err_returns_op_err() {
    let called = std::cell::Cell::new(false);
    let out: Result<()> = settle_with_restart(
        Err(anyhow::anyhow!("op failed")),
        restart_stub(true, &called),
    );
    assert_eq!(out.unwrap_err().to_string(), "op failed");
    assert!(called.get());
}

#[tokio::test]
async fn unmanaged_guard_passes_result_through() {
    // Unmanaged never touches real Steam.
    let ok = with_steam_stopped(false, true, async { Ok(5) }).await;
    assert_eq!(ok.unwrap(), 5);
    let err: Result<()> = with_steam_stopped(false, true, async { bail!("boom") }).await;
    assert_eq!(err.unwrap_err().to_string(), "boom");
}
