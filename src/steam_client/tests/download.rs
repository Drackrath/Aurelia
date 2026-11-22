use super::super::*;

#[test]
fn failed_progress_keeps_error_kind() {
    use crate::core::error::{ErrorKind, TypedError};
    use crate::core::models::DownloadProgress;
    let io = std::io::Error::from(std::io::ErrorKind::StorageFull);
    let p = DownloadProgress::failed_with("depot 1", &anyhow::Error::new(io));
    assert_eq!(p.error_kind, Some(ErrorKind::DiskFull));
    assert!(p.current_file.starts_with("depot 1: "));
    let stop = anyhow::anyhow!(TypedError::new(ErrorKind::Cancelled, "stopped"));
    assert_eq!(
        DownloadProgress::failed_with("x", &stop).error_kind,
        Some(ErrorKind::Cancelled)
    );
    // Unknown stays unset.
    assert_eq!(
        DownloadProgress::failed_with("x", &anyhow::anyhow!("odd")).error_kind,
        None
    );
    assert_eq!(
        DownloadProgress::failed("No space left on device (os error 28)").error_kind,
        Some(ErrorKind::DiskFull)
    );
    assert_eq!(
        DownloadProgress::failed("No matching depots").error_kind,
        None
    );
}

#[test]
fn cdn_errors_map_to_kinds() {
    use crate::core::error::ErrorKind;
    use install::cdn_error_kind;
    let full = steam_cdn::Error::Io(std::io::Error::from(std::io::ErrorKind::StorageFull));
    assert_eq!(cdn_error_kind(&full), Some(ErrorKind::DiskFull));
    let dns = steam_cdn::Error::Request(
        "error sending request: client error (Connect): dns error: failed to lookup address".into(),
    );
    assert_eq!(cdn_error_kind(&dns), Some(ErrorKind::Offline));
    let status = steam_cdn::Error::HttpStatus(reqwest::StatusCode::NOT_FOUND);
    assert_eq!(cdn_error_kind(&status), None);
}

#[test]
fn cdn_pool_is_never_empty() {
    let hosts = vec![
        content::content_host("CDN", "cache1.steamcontent.com", "mandatory"),
        content::content_host("SteamCache", "cache2.steamcontent.com", "mandatory"),
    ];
    // Missing tokens default to none.
    let pool = content::cdn_pool(&hosts, 7, vec![Some("t".into())]).unwrap();
    assert_eq!(pool.len(), 2);
    assert_eq!(pool[0].auth_token.as_deref(), Some("t"));
    assert_eq!(pool[1].auth_token, None);
    assert!(pool.iter().all(|s| s.cell_id == 7 && s.port == 443));
    // No hosts: error, not empty pool.
    assert!(content::cdn_pool(&[], 7, Vec::new()).is_err());
}

#[test]
fn cdn_server_parses_host_and_port() {
    let host = |h: &str, https| ContentHost {
        host: h.into(),
        https,
        needs_token: false,
    };
    let s = install::cdn_server_for(&host("cache1.steamcontent.com", false), 7, Some("t".into()));
    assert_eq!(
        (s.host.as_str(), s.port, s.https),
        ("cache1.steamcontent.com", 80, false)
    );
    assert_eq!(s.auth_token.as_deref(), Some("t"));
    let s = install::cdn_server_for(&host("cdn.example:443", false), 7, None);
    assert_eq!(
        (s.host.as_str(), s.port, s.https),
        ("cdn.example", 443, true)
    );
    let s = install::cdn_server_for(&host("cache2.steamcontent.com", true), 7, None);
    assert_eq!(
        (s.host.as_str(), s.port, s.https),
        ("cache2.steamcontent.com", 443, true)
    );
}

#[test]
fn https_support_values_map_to_tls() {
    assert!(content::supports_https("mandatory"));
    assert!(content::supports_https("optional"));
    assert!(!content::supports_https("none"));
    assert!(!content::supports_https(""));
}

#[test]
fn https_hosts_are_preferred() {
    let host = |h: &str, https| ContentHost {
        host: h.into(),
        https,
        needs_token: false,
    };
    let kept = content::prefer_https(vec![host("a", false), host("b", true), host("c", true)]);
    assert_eq!(kept, vec![host("b", true), host("c", true)]);
    let only_http = vec![host("a", false)];
    assert_eq!(content::prefer_https(only_http.clone()), only_http);
}

#[test]
fn only_cdn_hosts_need_tokens() {
    let cache = content::content_host("SteamCache", "c.example", "mandatory");
    let cdn = content::content_host("CDN", "d.example", "none");
    assert!(!cache.needs_token && cache.https);
    assert!(cdn.needs_token && !cdn.https);
    // HTTPS preference keeps the flag.
    let kept = content::prefer_https(vec![cache.clone(), cdn]);
    assert_eq!(kept, vec![cache]);
}

#[tokio::test]
async fn steam_cache_hosts_skip_the_token_request() {
    // No connection: any request would error.
    let client = SteamClient::new().unwrap();
    let cache = content::content_host("SteamCache", "c.example", "mandatory");
    assert_eq!(client.cdn_token_for(1, 2, &cache).await, None);
    let cdn = content::content_host("CDN", "d.example", "mandatory");
    assert_eq!(client.cdn_token_for(1, 2, &cdn).await, None);
}
