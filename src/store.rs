//! Small text helper shared by the metadata path.
//!
//! Aurelia's game metadata used to come from the HTTPS Steam storefront API
//! (`store.steampowered.com/api/appdetails`) and SteamSpy. That has been replaced
//! by the `StoreBrowse` service over the Steam CM connection (see
//! [`crate::steam_client::SteamClient::fetch_store_apps`]), so only this HTML
//! sanitiser remains here — store descriptions returned by Steam are HTML.

/// Strip HTML tags and decode the handful of entities Steam descriptions use,
/// so text renders cleanly in a terminal.
pub fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("\r\n", "\n")
        .trim()
        .to_string()
}
