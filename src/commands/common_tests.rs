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
