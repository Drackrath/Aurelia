use super::super::*;

#[test]
fn qr_poll_unscanned_expiry_regenerates() {
    use client::{QrPollStep, qr_poll_step};
    assert!(matches!(
        qr_poll_step(steam_vent::EResult::Expired, false),
        Some(QrPollStep::Regenerate)
    ));
}

#[test]
fn qr_poll_scanned_expiry_is_expired() {
    use client::{QrPollStep, qr_poll_step};
    match qr_poll_step(steam_vent::EResult::Expired, true) {
        Some(QrPollStep::Fail(e)) => {
            assert_eq!(e.kind, crate::core::error::ErrorKind::LoginExpired)
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn qr_poll_scanned_cancel_is_denied() {
    use client::{QrPollStep, qr_poll_step};
    for r in [
        steam_vent::EResult::FileNotFound,
        steam_vent::EResult::Cancelled,
    ] {
        match qr_poll_step(r, true) {
            Some(QrPollStep::Fail(e)) => {
                assert_eq!(e.kind, crate::core::error::ErrorKind::LoginDenied)
            }
            other => panic!("{r:?}: {other:?}"),
        }
    }
}

// Seen live: ~10 min unscanned.
#[test]
fn qr_poll_unscanned_session_end_regenerates() {
    use client::{QrPollStep, qr_poll_step};
    for r in [
        steam_vent::EResult::FileNotFound,
        steam_vent::EResult::Cancelled,
    ] {
        assert!(
            matches!(qr_poll_step(r, false), Some(QrPollStep::Regenerate)),
            "{r:?}"
        );
    }
}

#[test]
fn qr_poll_other_results_pass_through() {
    assert!(client::qr_poll_step(steam_vent::EResult::Busy, true).is_none());
}

#[tokio::test]
async fn guard_code_is_fed_through_duplex() {
    use tokio::io::AsyncBufReadExt;
    let (writer, reader) = tokio::io::duplex(64);
    let work = async move {
        let mut line = String::new();
        tokio::io::BufReader::new(reader)
            .read_line(&mut line)
            .await
            .unwrap();
        line
    };
    let code: CodeReader = Box::pin(async { Ok(Some(" 12345 ".to_string())) });
    let got = client::feed_code_while(work, code, writer).await;
    assert_eq!(got, "12345\n");
}

#[tokio::test]
async fn login_finishing_first_stops_waiting_for_code() {
    let (writer, _reader) = tokio::io::duplex(64);
    // Never yields, like idle stdin.
    let code: CodeReader = Box::pin(std::future::pending());
    let got = client::feed_code_while(async { 7 }, code, writer).await;
    assert_eq!(got, 7);
}

#[tokio::test]
async fn stdin_eof_ends_reader() {
    use tokio::io::AsyncReadExt;
    let (writer, mut reader) = tokio::io::duplex(64);
    let work = async move {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        buf.len()
    };
    let code: CodeReader = Box::pin(async { Ok(None) });
    assert_eq!(client::feed_code_while(work, code, writer).await, 0);
}
