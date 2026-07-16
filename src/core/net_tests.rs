use super::*;

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
