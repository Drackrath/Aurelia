use super::super::*;

#[test]
fn achievement_icon_urls() {
    assert_eq!(achievement_icon_url(440, ""), "");
    assert_eq!(
        achievement_icon_url(440, "abc123.jpg"),
        "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/440/abc123.jpg"
    );
    // An already-absolute URL is passed through unchanged.
    assert_eq!(
        achievement_icon_url(440, "https://example.com/i.png"),
        "https://example.com/i.png"
    );
}
