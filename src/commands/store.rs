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

/// Store record, refetched when cached pre-S3 (no media).
async fn store_info_with_media(app_id: u32) -> Result<StoreAppInfo> {
    use aurelia::core::config::{info_cache_ttl, load_info_cache};
    let lang = resolve_steam_language(None).await;
    let country = resolve_steam_country(None).await?;
    if let Some(cached) = load_info_cache(app_id, &lang, &country, info_cache_ttl()).await {
        if !cached.details.screenshots.is_empty() || !cached.details.tags.is_empty() {
            return Ok(cached.details);
        }
    }
    let client = authed_client().await?;
    client
        .fetch_store_apps(&[app_id], &lang, &country)
        .await?
        .into_iter()
        .find(|a| a.app_id == app_id)
        .ok_or_else(|| {
            TypedError::new(
                ErrorKind::NotFound,
                format!("no store information available for app {app_id}"),
            )
            .into()
        })
}

/// HEAD each URL, eight at a time.
async fn probe_urls(urls: &[String]) -> Vec<Option<u16>> {
    let Ok(client) = aurelia::core::net::http_client(Duration::from_secs(15)) else {
        return vec![None; urls.len()];
    };
    let mut statuses = vec![None; urls.len()];
    let indices: Vec<usize> = (0..urls.len()).collect();
    for chunk in indices.chunks(8) {
        let mut set = tokio::task::JoinSet::new();
        for &i in chunk {
            let client = client.clone();
            let url = urls[i].clone();
            set.spawn(async move {
                let status = aurelia::core::net::send_with_retry(&client, client.head(&url))
                    .await
                    .ok()
                    .map(|r| r.status().as_u16());
                (i, status)
            });
        }
        while let Some(Ok((i, status))) = set.join_next().await {
            statuses[i] = status;
        }
    }
    statuses
}

/// `aurelia image APPID --list [--probe]`: every asset URL.
pub(crate) async fn cmd_image_list(app_id: u32, probe: bool, json: bool) -> Result<()> {
    let info = store_info_with_media(app_id).await?;
    let mut entries: Vec<(&str, String)> = Vec::new();
    let a = &info.assets;
    for (kind, url) in [
        ("header", &a.header),
        ("capsule", &a.capsule),
        ("hero", &a.hero),
        ("background", &a.background),
        ("logo", &a.logo),
    ] {
        if let Some(u) = url {
            entries.push((kind, u.clone()));
        }
    }
    entries.extend(info.screenshots.iter().map(|u| ("screenshot", u.clone())));
    for t in &info.trailers {
        if let Some(u) = &t.thumbnail {
            entries.push(("trailer_thumb", u.clone()));
        }
        if let Some(u) = &t.url {
            entries.push(("trailer", u.clone()));
        }
    }

    let urls: Vec<String> = entries.iter().map(|(_, u)| u.clone()).collect();
    let statuses = if probe {
        probe_urls(&urls).await
    } else {
        vec![None; entries.len()]
    };

    if json {
        let items: Vec<serde_json::Value> = entries
            .iter()
            .zip(&statuses)
            .map(|((kind, url), status)| serde_json::json!({"kind": kind, "url": url, "status": status}))
            .collect();
        print_json(&serde_json::json!({
            "app_id": app_id,
            "name": info.name,
            "probed": probe,
            "assets": items,
        }));
        return Ok(());
    }

    cli_println!("{}  (app {app_id}): {} assets", info.name, entries.len());
    for ((kind, url), status) in entries.iter().zip(&statuses) {
        let status = match status {
            Some(s) => s.to_string(),
            None if probe => "ERR".to_string(),
            None => "-".to_string(),
        };
        cli_println!("{kind:<14} {status:<4} {url}");
    }
    if probe {
        let bad = statuses.iter().filter(|s| !matches!(s, Some(200..=299))).count();
        cli_println!("\n{} of {} URLs failed the probe", bad, entries.len());
    }
    Ok(())
}

/// Fetch full records for query ids, preserving order.
async fn store_records(
    client: &aurelia::steam_client::SteamClient,
    ids: &[u32],
    lang: &str,
    country: &str,
) -> Result<Vec<StoreAppInfo>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut records = client.fetch_store_apps(ids, lang, country).await?;
    let position = |id: u32| ids.iter().position(|&x| x == id).unwrap_or(usize::MAX);
    records.sort_by_key(|r| position(r.app_id));
    Ok(records)
}

/// Compact JSON row for listings.
fn store_row_json(a: &StoreAppInfo) -> serde_json::Value {
    serde_json::json!({
        "app_id": a.app_id,
        "name": a.name,
        "type": a.app_type,
        "is_free": a.is_free,
        "price": a.price,
        "price_cents": a.price_cents,
        "original_price": a.original_price,
        "discount_pct": a.discount_pct,
        "discount_end_date": a.discount_end.map(|t| unix_to_ymd(t as i64)),
        "release_date": a.release_date,
        "platforms": a.platforms,
        "reviews": a.review_summary,
        "country": a.country,
    })
}

/// Human table for listings.
fn print_store_table(records: &[StoreAppInfo]) {
    cli_println!("{:>9}  {:<14} {:<9} {:<11} NAME", "APPID", "PRICE", "DISCOUNT", "ENDS");
    for a in records {
        let price = a.price.clone().unwrap_or_else(|| "-".to_string());
        let discount = if a.discount_pct > 0 {
            format!("-{}%", a.discount_pct)
        } else {
            "-".to_string()
        };
        let ends = a.discount_end.map(|t| unix_to_ymd(t as i64)).unwrap_or_else(|| "-".to_string());
        let kind = if a.app_type.is_empty() || a.app_type == "Game" {
            String::new()
        } else {
            format!("  [{}]", a.app_type)
        };
        cli_println!("{:>9}  {:<14} {:<9} {:<11} {}{kind}", a.app_id, price, discount, ends, a.name);
    }
}

/// `aurelia search TERM`: find app ids by title.
pub(crate) async fn cmd_search(
    term: String,
    count: u32,
    country: Option<String>,
    lang: Option<String>,
    json: bool,
) -> Result<()> {
    let lang = resolve_steam_language(lang).await;
    let country = resolve_steam_country(country).await?;
    let client = authed_client().await?;
    let page = client.search_store(&term, count, &lang, &country).await?;
    let records = store_records(&client, &page.app_ids, &lang, &country).await?;
    if json {
        print_json(&serde_json::json!({
            "term": term,
            "total": page.total,
            "suggestions": page.suggestions,
            "results": records.iter().map(store_row_json).collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    if records.is_empty() {
        cli_println!("No store results for \"{term}\".");
        if !page.suggestions.is_empty() {
            cli_println!("Did you mean: {}", page.suggestions.join(", "));
        }
        return Ok(());
    }
    print_store_table(&records);
    Ok(())
}

/// `aurelia deals`: discounted and top-selling games.
pub(crate) async fn cmd_deals(
    scope: crate::cli::DealsScopeArg,
    min_discount: i32,
    count: i32,
    start: i32,
    country: Option<String>,
    lang: Option<String>,
    json: bool,
) -> Result<()> {
    use aurelia::steam_client::DealsScope;
    let lang = resolve_steam_language(lang).await;
    let country = resolve_steam_country(country).await?;
    let scope = match scope {
        crate::cli::DealsScopeArg::DiscountedTopSellers => DealsScope::DiscountedTopSellers,
        crate::cli::DealsScopeArg::TopSellers => DealsScope::TopSellers,
        crate::cli::DealsScopeArg::Specials => DealsScope::Specials,
    };
    let client = authed_client().await?;
    let page = client
        .query_deals(scope, min_discount, start, count, &lang, &country)
        .await?;
    let records = store_records(&client, &page.app_ids, &lang, &country).await?;
    if json {
        print_json(&serde_json::json!({
            "country": country,
            "scope": format!("{scope:?}"),
            "start": start,
            "total": page.total,
            "results": records.iter().map(store_row_json).collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    if records.is_empty() {
        cli_println!("No deals matched in {country}.");
        return Ok(());
    }
    cli_println!("{scope:?} in {country} ({} matching, showing from {start}):", page.total);
    print_store_table(&records);
    Ok(())
}

/// `aurelia similar APPID`: related games.
pub(crate) async fn cmd_similar(
    app_id: u32,
    count: i32,
    country: Option<String>,
    lang: Option<String>,
    json: bool,
) -> Result<()> {
    let lang = resolve_steam_language(lang).await;
    let country = resolve_steam_country(country).await?;
    let client = authed_client().await?;
    let page = client.similar_apps(app_id, count, &lang, &country).await?;
    let records = store_records(&client, &page.app_ids, &lang, &country).await?;
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "results": records.iter().map(store_row_json).collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    if records.is_empty() {
        cli_println!("No similar games reported for app {app_id}.");
        return Ok(());
    }
    print_store_table(&records);
    Ok(())
}

/// `aurelia players APPID`: current in-game count.
pub(crate) async fn cmd_players(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let players = client.current_players(app_id).await?;
    if json {
        print_json(&serde_json::json!({ "app_id": app_id, "players": players }));
    } else {
        cli_println!("{players} players in-game (app {app_id})");
    }
    Ok(())
}

/// `aurelia events`: active store sales and events.
pub(crate) async fn cmd_events(country: Option<String>, json: bool) -> Result<()> {
    let country = resolve_steam_country(country).await?;
    let client = authed_client().await?;
    let events = client.active_store_events(&country).await?;
    if json {
        print_json(&serde_json::json!({ "country": country, "events": events }));
        return Ok(());
    }
    if events.is_empty() {
        cli_println!("No active store events for {country}.");
        return Ok(());
    }
    cli_println!("{:<12} {:<11} {:<11} TITLE", "TYPE", "START", "END");
    for e in &events {
        let start = if e.start > 0 { unix_to_ymd(e.start as i64) } else { "-".to_string() };
        let end = if e.end > 0 { unix_to_ymd(e.end as i64) } else { "-".to_string() };
        let assoc = if e.associated_name.is_empty() {
            String::new()
        } else {
            format!("  ({})", e.associated_name)
        };
        cli_println!("{:<12} {start:<11} {end:<11} {}{assoc}", e.kind, e.title);
    }
    Ok(())
}

/// `aurelia news APPID`: announcements from the storefront feed.
pub(crate) async fn cmd_news(app_id: u32, count: u32, max_length: u32, json: bool) -> Result<()> {
    let http = aurelia::core::net::steam_web_client()?;
    let items = aurelia::web::discovery::fetch_news(&http, app_id, count, max_length).await?;
    if json {
        print_json(&serde_json::json!({ "app_id": app_id, "news": items }));
        return Ok(());
    }
    if items.is_empty() {
        cli_println!("No news for app {app_id}.");
        return Ok(());
    }
    for n in &items {
        cli_println!("{}  {}  [{}]", unix_to_ymd(n.date as i64), n.title, n.feed);
        cli_println!("  {}", n.url);
        if !n.contents.is_empty() {
            cli_println!("  {}", n.contents.replace('\n', " "));
        }
        cli_println!();
    }
    Ok(())
}

/// `aurelia reviews APPID`: one page of user reviews.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn cmd_reviews(
    app_id: u32,
    filter: String,
    review_type: String,
    purchase: String,
    count: u32,
    cursor: Option<String>,
    lang: Option<String>,
    json: bool,
) -> Result<()> {
    use aurelia::web::discovery::{fetch_reviews, ReviewQuery};
    let language = match lang {
        Some(l) => l,
        None => resolve_steam_language(None).await,
    };
    let query = ReviewQuery {
        filter,
        review_type,
        purchase_type: purchase,
        language,
        count,
        cursor,
    };
    let http = aurelia::core::net::steam_web_client()?;
    let page = fetch_reviews(&http, app_id, &query).await?;
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "summary": page.summary,
            "reviews": page.reviews,
            "next_cursor": page.next_cursor,
        }));
        return Ok(());
    }
    // Steam omits totals unless `--filter all`.
    let s = &page.summary;
    if s.total_reviews > 0 {
        cli_println!(
            "{} ({}% positive, {} reviews: {} up / {} down)",
            s.label,
            s.total_positive * 100 / s.total_reviews,
            s.total_reviews,
            s.total_positive,
            s.total_negative
        );
    }
    for r in &page.reviews {
        let verdict = if r.voted_up { "👍" } else { "👎" };
        cli_println!(
            "\n{verdict} {}  {}  {:.1}h played  {} helpful",
            unix_to_ymd(r.created as i64),
            r.author,
            r.playtime_hours,
            r.votes_up
        );
        for line in r.text.lines().take(6) {
            cli_println!("   {}", truncate(line, 110));
        }
    }
    if let Some(c) = &page.next_cursor {
        cli_println!("\nNext page: --cursor '{c}'");
    }
    Ok(())
}

/// `aurelia wishlist [USER]`: a wishlist with store records.
pub(crate) async fn cmd_wishlist(
    user: Option<String>,
    count: usize,
    offset: usize,
    country: Option<String>,
    lang: Option<String>,
    json: bool,
) -> Result<()> {
    let lang = resolve_steam_language(lang).await;
    let country = resolve_steam_country(country).await?;
    let client = authed_client().await?;
    let own_id = client.steam_id();
    let steam_id = match &user {
        Some(u) => resolve_user_ident(&client, u).await?,
        None => own_id.ok_or_else(|| {
            TypedError::new(ErrorKind::AuthRequired, "not logged in — run `aurelia login` first")
        })?,
    };
    let is_own = own_id == Some(steam_id);
    // Steam only serves friends' wishlists over the CM.
    let entries = client.wishlist(steam_id).await.map_err(|e| {
        if aurelia::core::error::classify(&e).kind == ErrorKind::AccessDenied {
            TypedError::new(
                ErrorKind::PrivacyRestricted,
                format!("Steam does not expose the wishlist of {steam_id} to you (only friends' wishlists are readable)"),
            )
            .into()
        } else {
            e
        }
    })?;
    if entries.is_empty() && !is_own {
        return Err(TypedError::new(
            ErrorKind::PrivacyRestricted,
            format!("no wishlist items for {steam_id}: the wishlist may be private, friends-only, or empty"),
        )
        .into());
    }
    let total = entries.len();
    let page: Vec<_> = entries.into_iter().skip(offset).take(count).collect();
    let ids: Vec<u32> = page.iter().map(|e| e.app_id).collect();
    let records = store_records(&client, &ids, &lang, &country).await?;
    let record_for = |id: u32| records.iter().find(|r| r.app_id == id);

    if json {
        let items: Vec<serde_json::Value> = page
            .iter()
            .map(|e| {
                let mut v = record_for(e.app_id).map(store_row_json).unwrap_or_else(|| {
                    serde_json::json!({ "app_id": e.app_id, "name": null })
                });
                v["priority"] = e.priority.into();
                v["date_added"] = e.date_added.into();
                v["date_added_date"] = unix_to_ymd(e.date_added as i64).into();
                v
            })
            .collect();
        print_json(&serde_json::json!({
            "steam_id": steam_id,
            "total": total,
            "offset": offset,
            "items": items,
        }));
        return Ok(());
    }

    if total == 0 {
        cli_println!("Your wishlist is empty.");
        return Ok(());
    }
    cli_println!(
        "Wishlist of {steam_id}: {total} item(s), showing {}–{}",
        offset + 1,
        offset + page.len()
    );
    cli_println!("{:>4}  {:>9}  {:<14} {:<9} {:<11} NAME", "#", "APPID", "PRICE", "DISCOUNT", "ADDED");
    for e in &page {
        let rank = if e.priority > 0 { e.priority.to_string() } else { "-".to_string() };
        let (name, price, discount) = match record_for(e.app_id) {
            Some(r) => (
                r.name.clone(),
                r.price.clone().unwrap_or_else(|| "-".to_string()),
                if r.discount_pct > 0 { format!("-{}%", r.discount_pct) } else { "-".to_string() },
            ),
            None => ("(not on the store)".to_string(), "-".to_string(), "-".to_string()),
        };
        cli_println!(
            "{rank:>4}  {:>9}  {price:<14} {discount:<9} {:<11} {name}",
            e.app_id,
            unix_to_ymd(e.date_added as i64)
        );
    }
    Ok(())
}

/// `aurelia wishlist add APPID`.
pub(crate) async fn cmd_wishlist_add(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let count = client.wishlist_add(app_id).await?;
    if json {
        print_json(&serde_json::json!({ "app_id": app_id, "status": "added", "wishlist_count": count }));
    } else {
        cli_println!("Added app {app_id} to your wishlist ({count} items).");
    }
    Ok(())
}

/// `aurelia wishlist remove APPID`.
pub(crate) async fn cmd_wishlist_remove(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let count = client.wishlist_remove(app_id).await?;
    if json {
        print_json(&serde_json::json!({ "app_id": app_id, "status": "removed", "wishlist_count": count }));
    } else {
        cli_println!("Removed app {app_id} from your wishlist ({count} items).");
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
