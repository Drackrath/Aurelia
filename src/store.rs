//! Steam Storefront / SteamSpy lookups for the storefront-only fields shown by
//! `aurelia info --extended`: system requirements, Metacritic, website, store
//! genres/categories, and SteamSpy user tags. These have no equivalent in the
//! `StoreBrowse` CM protocol (which returns only numeric tag/category ids), so
//! they are fetched from the public HTTPS storefront — the one part of the
//! metadata path that still uses the web API, and only when `--extended` is set.
//!
//! The default `info` path is protocol-native via
//! [`crate::steam_client::SteamClient::fetch_store_apps`].

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

/// Human-facing details about a game, assembled from the Steam storefront API.
#[derive(Debug, Clone)]
pub struct AppDetails {
    pub app_id: u32,
    pub name: String,
    pub app_type: String,
    pub is_free: bool,
    pub short_description: String,
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    pub release_date: Option<String>,
    pub coming_soon: bool,
    pub price: Option<String>,
    pub platforms: Vec<String>,
    pub categories: Vec<String>,
    pub genres: Vec<String>,
    pub metacritic: Option<i64>,
    pub website: Option<String>,
    pub dlc: Vec<u32>,
    /// Minimum hardware/system requirements, one "Label: value" entry per line.
    pub requirements_minimum: Vec<String>,
    /// Recommended hardware/system requirements, one entry per line.
    pub requirements_recommended: Vec<String>,
}

// ---- Raw storefront JSON shapes -------------------------------------------------

#[derive(Deserialize)]
struct Envelope {
    success: bool,
    data: Option<RawAppData>,
}

#[derive(Deserialize)]
struct RawAppData {
    #[serde(default)]
    name: String,
    #[serde(rename = "type", default)]
    app_type: String,
    #[serde(default)]
    is_free: bool,
    #[serde(default)]
    short_description: String,
    #[serde(default)]
    developers: Vec<String>,
    #[serde(default)]
    publishers: Vec<String>,
    #[serde(default)]
    release_date: Option<ReleaseDate>,
    #[serde(default)]
    price_overview: Option<PriceOverview>,
    #[serde(default)]
    platforms: Option<Platforms>,
    #[serde(default)]
    categories: Vec<Described>,
    #[serde(default)]
    genres: Vec<Described>,
    #[serde(default)]
    metacritic: Option<Metacritic>,
    #[serde(default)]
    website: Option<String>,
    #[serde(default)]
    dlc: Vec<u32>,
    // Steam returns an object {minimum, recommended} or an empty array `[]`;
    // keeping this as a Value tolerates both shapes.
    #[serde(default)]
    pc_requirements: serde_json::Value,
    #[serde(default)]
    linux_requirements: serde_json::Value,
    #[serde(default)]
    mac_requirements: serde_json::Value,
}

#[derive(Deserialize)]
struct Described {
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct ReleaseDate {
    #[serde(default)]
    coming_soon: bool,
    #[serde(default)]
    date: String,
}

#[derive(Deserialize)]
struct PriceOverview {
    #[serde(default)]
    final_formatted: String,
}

#[derive(Deserialize)]
struct Platforms {
    #[serde(default)]
    windows: bool,
    #[serde(default)]
    mac: bool,
    #[serde(default)]
    linux: bool,
}

#[derive(Deserialize)]
struct Metacritic {
    #[serde(default)]
    score: i64,
}

/// Fetch storefront details for a single app. Returns `Ok(None)` if the store has
/// no public entry for the app id (e.g. it was delisted).
pub async fn fetch_app_details(
    client: &reqwest::Client,
    app_id: u32,
    language: &str,
) -> Result<Option<AppDetails>> {
    let url = format!(
        "https://store.steampowered.com/api/appdetails?appids={app_id}&l={language}&cc=us"
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed requesting store details for app {app_id}"))?;

    let map: HashMap<String, Envelope> = resp
        .json()
        .await
        .with_context(|| format!("failed parsing store details for app {app_id}"))?;

    let Some(env) = map.get(&app_id.to_string()) else {
        return Ok(None);
    };
    if !env.success {
        return Ok(None);
    }
    let Some(data) = &env.data else {
        return Ok(None);
    };

    let mut platforms = Vec::new();
    if let Some(p) = &data.platforms {
        if p.windows {
            platforms.push("Windows".to_string());
        }
        if p.linux {
            platforms.push("Linux".to_string());
        }
        if p.mac {
            platforms.push("macOS".to_string());
        }
    }

    // Prefer Windows requirements; fall back to Linux/macOS if a title is
    // native-only on those platforms.
    let requirements = [
        &data.pc_requirements,
        &data.linux_requirements,
        &data.mac_requirements,
    ]
    .into_iter()
    .find(|v| v.get("minimum").is_some() || v.get("recommended").is_some());

    let lines = |key| {
        requirements
            .and_then(|req| req.get(key))
            .and_then(|v| v.as_str())
            .map(requirements_lines)
            .unwrap_or_default()
    };
    let (requirements_minimum, requirements_recommended) = (lines("minimum"), lines("recommended"));

    Ok(Some(AppDetails {
        app_id,
        name: data.name.clone(),
        app_type: data.app_type.clone(),
        is_free: data.is_free,
        short_description: strip_html(&data.short_description),
        developers: data.developers.clone(),
        publishers: data.publishers.clone(),
        release_date: data
            .release_date
            .as_ref()
            .map(|r| r.date.clone())
            .filter(|d| !d.is_empty()),
        coming_soon: data.release_date.as_ref().is_some_and(|r| r.coming_soon),
        price: if data.is_free {
            Some("Free".to_string())
        } else {
            data.price_overview
                .as_ref()
                .map(|p| p.final_formatted.clone())
                .filter(|s| !s.is_empty())
        },
        platforms,
        categories: data.categories.iter().map(|c| c.description.clone()).collect(),
        genres: data.genres.iter().map(|g| g.description.clone()).collect(),
        metacritic: data.metacritic.as_ref().map(|m| m.score),
        website: data.website.clone().filter(|s| !s.is_empty()),
        dlc: data.dlc.clone(),
        requirements_minimum,
        requirements_recommended,
    }))
}

/// Convert a requirements HTML blob into clean "Label: value" lines, dropping the
/// leading "Minimum:"/"Recommended:" heading (callers render their own).
fn requirements_lines(html: &str) -> Vec<String> {
    // Turn list items and line breaks into newlines before stripping tags.
    let normalized = html
        .replace("</li>", "\n")
        .replace("</ul>", "\n")
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");

    strip_html(&normalized)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.eq_ignore_ascii_case("Minimum:") && !line.eq_ignore_ascii_case("Recommended:"))
        .map(String::from)
        .collect()
}

/// Fetch community/user tags for an app from SteamSpy, ordered by popularity.
/// Best-effort: returns an empty list on any error or if no tags are available.
pub async fn fetch_tags(client: &reqwest::Client, app_id: u32) -> Vec<String> {
    let url = format!("https://steamspy.com/api.php?request=appdetails&appid={app_id}");
    let Ok(resp) = client.get(&url).send().await else {
        return Vec::new();
    };
    let Ok(value) = resp.json::<serde_json::Value>().await else {
        return Vec::new();
    };

    // `tags` is either an object {name: votes} or an empty array when absent.
    let Some(obj) = value.get("tags").and_then(|t| t.as_object()) else {
        return Vec::new();
    };

    let mut tags: Vec<(String, i64)> = obj
        .iter()
        .map(|(name, votes)| (name.clone(), votes.as_i64().unwrap_or(0)))
        .collect();
    tags.sort_by(|a, b| b.1.cmp(&a.1));
    tags.into_iter().map(|(name, _)| name).collect()
}

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
