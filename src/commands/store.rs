//! Store price lookups (`price`).

use crate::commands::common::*;

use anyhow::Result;
use aurelia::core::error::{ErrorKind, TypedError};
use aurelia::core::locale::normalize_country;
use aurelia::steam_client::{unix_to_ymd, StoreAppInfo, StorePurchaseOption};
use std::time::Duration;

/// Spacing between per-region StoreBrowse calls.
const REGION_SPACING: Duration = Duration::from_millis(150);

/// One region's quote for an app.
#[derive(Debug, serde::Serialize)]
struct PriceQuote {
    country: String,
    available: bool,
    is_free: bool,
    price: Option<String>,
    price_cents: Option<i64>,
    original_price: Option<String>,
    original_price_cents: Option<i64>,
    discount_pct: i32,
    discount_end: Option<u64>,
    discount_end_date: Option<String>,
    purchase_options: Vec<StorePurchaseOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl PriceQuote {
    fn from_info(info: &StoreAppInfo) -> Self {
        Self {
            country: info.country.clone(),
            available: !info.region_locked && (info.is_free || info.price.is_some()),
            is_free: info.is_free,
            price: info.price.clone(),
            price_cents: info.price_cents,
            original_price: info.original_price.clone(),
            original_price_cents: info.original_price_cents,
            discount_pct: info.discount_pct,
            discount_end: info.discount_end,
            discount_end_date: info.discount_end.map(|t| unix_to_ymd(t as i64)),
            purchase_options: info.purchase_options.clone(),
            error: None,
        }
    }

    fn unavailable(country: &str, error: Option<String>) -> Self {
        Self {
            country: country.to_string(),
            available: false,
            is_free: false,
            price: None,
            price_cents: None,
            original_price: None,
            original_price_cents: None,
            discount_pct: 0,
            discount_end: None,
            discount_end_date: None,
            purchase_options: Vec::new(),
            error,
        }
    }
}

/// `--compare` list, normalised and de-duplicated.
fn parse_regions(compare: &[String]) -> Result<Vec<String>> {
    let mut regions: Vec<String> = Vec::new();
    for raw in compare.iter().flat_map(|s| s.split(',')) {
        if raw.trim().is_empty() {
            continue;
        }
        let cc = normalize_country(raw).ok_or_else(|| {
            TypedError::new(
                ErrorKind::InvalidInput,
                format!("invalid country code `{raw}` — use two-letter ISO codes like US,DE,JP"),
            )
        })?;
        if !regions.contains(&cc) {
            regions.push(cc);
        }
    }
    Ok(regions)
}

/// `aurelia price APPID [--compare CC,…]`.
pub(crate) async fn cmd_price(
    app_id: u32,
    compare: Vec<String>,
    country: Option<String>,
    lang: Option<String>,
    json: bool,
) -> Result<()> {
    let lang = resolve_steam_language(lang).await;
    let mut regions = parse_regions(&compare)?;
    if regions.is_empty() {
        regions.push(resolve_steam_country(country).await?);
    }

    let client = authed_client().await?;
    let mut name: Option<String> = None;
    let mut quotes = Vec::with_capacity(regions.len());
    for (i, region) in regions.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(REGION_SPACING).await;
        }
        let quote = match client.fetch_store_apps(&[app_id], &lang, region).await {
            Ok(apps) => match apps.into_iter().find(|a| a.app_id == app_id) {
                Some(info) => {
                    if name.is_none() && !info.name.is_empty() {
                        name = Some(info.name.clone());
                    }
                    PriceQuote::from_info(&info)
                }
                None => PriceQuote::unavailable(region, None),
            },
            Err(e) => PriceQuote::unavailable(region, Some(format!("{e:#}"))),
        };
        quotes.push(quote);
    }

    if name.is_none() {
        return Err(TypedError::new(
            ErrorKind::NotFound,
            format!("no store information available for app {app_id}"),
        )
        .into());
    }

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "name": name,
            "regions": quotes,
        }));
        return Ok(());
    }

    cli_println!("{}  (app {app_id})", name.unwrap_or_default());
    cli_println!("{:<8} {:<14} {:<9} {:<14} {}", "Country", "Price", "Discount", "Original", "Ends");
    for q in &quotes {
        if !q.available {
            let why = q.error.as_deref().unwrap_or("not sold in this region");
            cli_println!("{:<8} {why}", q.country);
            continue;
        }
        let price = q.price.clone().unwrap_or_else(|| "Free".to_string());
        let discount = if q.discount_pct > 0 {
            format!("-{}%", q.discount_pct)
        } else {
            "-".to_string()
        };
        let original = q.original_price.clone().unwrap_or_else(|| "-".to_string());
        let ends = q.discount_end_date.clone().unwrap_or_else(|| "-".to_string());
        cli_println!("{:<8} {:<14} {:<9} {:<14} {ends}", q.country, price, discount, original);
    }

    // Packages and bundles, for the first priced region.
    if let Some(q) = quotes.iter().find(|q| q.available && q.purchase_options.len() > 1) {
        cli_println!("\nPurchase options [{}]:", q.country);
        for o in &q.purchase_options {
            let price = o.price.clone().unwrap_or_else(|| "-".to_string());
            let mut note = String::new();
            if o.discount_pct > 0 {
                note.push_str(&format!("  -{}%", o.discount_pct));
            }
            if o.bundle_discount_pct > 0 {
                note.push_str(&format!(" (bundle -{}%)", o.bundle_discount_pct));
            }
            if o.included_games > 1 {
                note.push_str(&format!("  {} items", o.included_games));
            }
            if let Some(end) = o.discount_end {
                note.push_str(&format!("  until {}", unix_to_ymd(end as i64)));
            }
            cli_println!("  {:<8} {:<44} {:<12}{note}", o.kind, truncate(&o.name, 44), price);
        }
    }
    Ok(())
}

/// `aurelia tags [--dump]`: the store tag vocabulary.
pub(crate) async fn cmd_tags(dump: bool, json: bool) -> Result<()> {
    crate::commands::auth::require_experimental("tags").await?;
    let client = authed_client().await?;
    let tags = client.fetch_tag_list("english").await?;
    if json {
        print_json(&tags.iter().map(|(id, name)| serde_json::json!({"id": id, "name": name})).collect::<Vec<_>>());
        return Ok(());
    }
    if !dump {
        for (id, name) in &tags {
            cli_println!("{id:>6}  {name}");
        }
        return Ok(());
    }
    let today = unix_to_ymd(aurelia::core::utils::now_unix() as i64);
    cli_println!("//! Generated by `aurelia tags --dump`; do not edit.\n");
    cli_println!("/// Snapshot date of the vocabulary below.");
    cli_println!("pub const TAGS_GENERATED_AT: &str = \"{today}\";\n");
    cli_println!("/// One store tag.\npub struct TagDef {{\n    pub id: u32,\n    pub name: &'static str,\n}}\n");
    cli_println!("/// Sorted by `id`.\npub static TAGS: &[TagDef] = &[");
    for (id, name) in &tags {
        cli_println!("    TagDef {{ id: {id}, name: {:?} }},", name);
    }
    cli_println!("];\n");
    cli_println!("/// English name for a tag id.\npub fn tag_name(id: u32) -> Option<&'static str> {{");
    cli_println!("    TAGS.binary_search_by_key(&id, |t| t.id)\n        .ok()\n        .map(|i| TAGS[i].name)\n}}");
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}
