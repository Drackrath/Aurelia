use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn validate_accepts_supported_schemes() {
    for url in [
        "http://host:8080",
        "http://user:pass@host:8080",
        "https://proxy.example:3128",
        "socks5://127.0.0.1:1080",
        "socks5h://127.0.0.1:1080",
    ] {
        assert!(validate_proxy_url(url).is_ok(), "should accept {url}");
    }
}

#[test]
fn validate_rejects_garbage() {
    assert!(validate_proxy_url("not a url").is_err());
}

/// Serve canned responses, one per connection.
async fn serve(responses: Vec<String>) -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        for body in responses {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            counter.fetch_add(1, Ordering::SeqCst);
            sock.write_all(body.as_bytes()).await.unwrap();
            sock.shutdown().await.ok();
        }
    });
    (url, hits)
}

fn response(status: &str, extra_headers: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\n{extra_headers}Content-Length: 2\r\nConnection: close\r\n\r\nok"
    )
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

#[tokio::test]
async fn short_retry_after_is_slept_once() {
    let (url, hits) = serve(vec![
        response("429 Too Many Requests", "Retry-After: 1\r\n"),
        response("200 OK", ""),
    ])
    .await;
    let started = std::time::Instant::now();
    let c = client();
    let resp = send_with_retry(&c, c.get(&url)).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert!(started.elapsed() >= Duration::from_secs(1));
}

#[tokio::test]
async fn bare_429_fails_fast_as_rate_limited() {
    let (url, hits) = serve(vec![response("429 Too Many Requests", "")]).await;
    let c = client();
    let err = send_with_retry(&c, c.get(&url)).await.unwrap_err();
    let classified = crate::core::error::classify(&err);
    assert_eq!(classified.kind, ErrorKind::RateLimited);
    assert_eq!(classified.retry_after, None);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn long_retry_after_is_reported_not_slept() {
    let (url, hits) =
        serve(vec![response("429 Too Many Requests", "Retry-After: 120\r\n")]).await;
    let c = client();
    let err = send_with_retry(&c, c.get(&url)).await.unwrap_err();
    let classified = crate::core::error::classify(&err);
    assert_eq!(classified.kind, ErrorKind::RateLimited);
    assert_eq!(classified.retry_after, Some(Duration::from_secs(120)));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn transient_5xx_is_retried() {
    let (url, hits) = serve(vec![
        response("503 Service Unavailable", ""),
        response("200 OK", ""),
    ])
    .await;
    let c = client();
    let resp = send_with_retry(&c, c.get(&url)).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn github_secondary_limit_maps_reset_header() {
    let reset = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 60;
    let (url, hits) = serve(vec![response(
        "403 Forbidden",
        &format!("X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: {reset}\r\n"),
    )])
    .await;
    let c = client();
    let err = send_with_retry(&c, c.get(&url)).await.unwrap_err();
    let classified = crate::core::error::classify(&err);
    assert_eq!(classified.kind, ErrorKind::RateLimited);
    let secs = classified.retry_after.unwrap().as_secs();
    assert!((55..=60).contains(&secs), "got {secs}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn plain_404_is_returned_to_caller() {
    let (url, hits) = serve(vec![response("404 Not Found", "")]).await;
    let c = client();
    let resp = send_with_retry(&c, c.get(&url)).await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
