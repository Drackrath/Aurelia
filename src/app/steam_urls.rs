//! Conventional Steam CDN / storefront URL builders.
//!
//! These are baked into the `--json` responses (`list`, `info`, `dlc`) so a
//! consuming driver (e.g. Heroic) gets ready-to-use artwork and store URLs
//! instead of hard-coding the base paths and reconstructing them from app ids.

const CDN_APPS_BASE: &str = "https://cdn.cloudflare.steamstatic.com/steam/apps";
const STORE_APP_BASE: &str = "https://store.steampowered.com/app";

/// Store page for an app (or DLC) id.
pub fn store_url(app_id: u32) -> String {
    format!("{STORE_APP_BASE}/{app_id}")
}

/// Wide store header / capsule image.
pub fn header_url(app_id: u32) -> String {
    format!("{CDN_APPS_BASE}/{app_id}/header.jpg")
}

/// Portrait library cover (`library_600x900`).
pub fn capsule_url(app_id: u32) -> String {
    format!("{CDN_APPS_BASE}/{app_id}/library_600x900.jpg")
}

/// Small horizontal capsule, used as a DLC artwork fallback.
pub fn small_capsule_url(app_id: u32) -> String {
    format!("{CDN_APPS_BASE}/{app_id}/capsule_231x87.jpg")
}

/// Wide library hero background.
pub fn hero_url(app_id: u32) -> String {
    format!("{CDN_APPS_BASE}/{app_id}/library_hero.jpg")
}

/// Transparent game logo.
pub fn logo_url(app_id: u32) -> String {
    format!("{CDN_APPS_BASE}/{app_id}/logo.png")
}

#[cfg(test)]
#[path = "steam_urls_tests.rs"]
mod tests;
