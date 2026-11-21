use super::super::*;

/// Offline reqwest status error, with URL.
fn status_error(code: u16) -> reqwest::Error {
    use reqwest::ResponseBuilderExt;
    let resp = http::Response::builder()
        .status(code)
        .url("https://cdn.example/ugc/1?sig=SECRET".parse().unwrap())
        .body(Vec::<u8>::new())
        .unwrap();
    reqwest::Response::from(resp)
        .error_for_status()
        .unwrap_err()
}

#[test]
fn transient_classification() {
    let e503 = http_error(status_error(503));
    assert_eq!(transfer_failure(&e503), TransferFailure::Status(503));
    assert!(is_transient(transfer_failure(&e503)));
    assert!(is_transient(TransferFailure::Status(429)));
    assert!(is_transient(TransferFailure::Timeout));
    assert!(is_transient(TransferFailure::Connect));
    assert!(!is_transient(transfer_failure(&http_error(status_error(
        404
    )))));
    assert!(!is_transient(transfer_failure(&anyhow!("rpc failed"))));
}

#[test]
fn http_errors_carry_no_signed_url() {
    let raw = status_error(500);
    assert!(raw.to_string().contains("SECRET"));
    let e = http_error(raw).context("cloud HTTP GET returned failure status");
    let text = format!("{e:#}");
    assert!(!text.contains("SECRET") && !text.contains('?'), "{text}");
}

#[test]
fn backoff_schedule_and_deadline_scaling() {
    assert_eq!(TRANSFER_BACKOFF.map(|d| d.as_secs()), [2, 4, 8]);
    assert_eq!(upload_deadline(10), UPLOAD_MIN_DEADLINE);
    // 8 MiB at 32 KiB/s.
    assert_eq!(upload_deadline(8 * 1024 * 1024), Duration::from_secs(256));
}

#[tokio::test]
async fn retry_stops_after_four_attempts() {
    let delays = [Duration::ZERO; 3];
    let mut calls = 0;
    let res: Result<()> = retry_transfer("t", &delays, |_| {
        calls += 1;
        async { Err(http_error(status_error(503))) }
    })
    .await;
    assert!(res.is_err());
    assert_eq!(calls, 4);
}

#[tokio::test]
async fn retry_fails_fast_on_client_error_and_recovers() {
    let delays = [Duration::ZERO; 3];
    let mut calls = 0;
    let res: Result<()> = retry_transfer("t", &delays, |_| {
        calls += 1;
        async { Err(http_error(status_error(403))) }
    })
    .await;
    assert!(res.is_err());
    assert_eq!(calls, 1);

    let mut calls = 0;
    let res = retry_transfer("t", &delays, |attempt| {
        calls += 1;
        async move {
            if attempt < 3 {
                Err(http_error(status_error(503)))
            } else {
                Ok(attempt)
            }
        }
    })
    .await;
    assert_eq!(res.unwrap(), 3);
    assert_eq!(calls, 3);
}
