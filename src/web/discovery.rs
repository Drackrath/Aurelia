//! Storefront JSON with no CM equivalent: reviews, news.

use crate::core::error::{ErrorKind, TypedError};
use crate::core::net::send_with_retry;
use anyhow::{Context, Result};
use serde_json::Value;

/// Aggregate review score for an app.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ReviewSummary {
    pub score: i64,
    pub label: String,
    pub total_positive: i64,
    pub total_negative: i64,
    pub total_reviews: i64,
}

/// One user review.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Review {
    pub id: String,
    pub author: String,
    pub steam_id: String,
    pub language: String,
    pub voted_up: bool,
    pub votes_up: i64,
    pub votes_funny: i64,
    pub playtime_hours: f64,
    pub playtime_at_review_hours: f64,
    pub created: u64,
    pub steam_purchase: bool,
    pub received_for_free: bool,
    pub early_access: bool,
    pub text: String,
}

/// A page of reviews plus the cursor for the next page.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ReviewPage {
    pub summary: ReviewSummary,
    pub reviews: Vec<Review>,
    pub next_cursor: Option<String>,
}

/// Query knobs for `appreviews`.
#[derive(Debug, Clone)]
pub struct ReviewQuery {
    /// `recent`, `updated` or `all`.
    pub filter: String,
    /// `all`, `positive` or `negative`.
    pub review_type: String,
    /// `all`, `steam` or `non_steam_purchase`.
    pub purchase_type: String,
    /// Steam API language name, or `all`.
    pub language: String,
    pub count: u32,
    pub cursor: Option<String>,
}

fn i64_of(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key)
        .map(|x| match x {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

fn bool_of(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Fetch one page of reviews from the storefront.
pub async fn fetch_reviews(
    client: &reqwest::Client,
    app_id: u32,
    query: &ReviewQuery,
) -> Result<ReviewPage> {
    let url = format!("https://store.steampowered.com/appreviews/{app_id}");
    let mut params: Vec<(&str, String)> = vec![
        ("json", "1".to_string()),
        ("filter", query.filter.clone()),
        ("review_type", query.review_type.clone()),
        ("purchase_type", query.purchase_type.clone()),
        ("language", query.language.clone()),
        ("num_per_page", query.count.min(100).to_string()),
    ];
    if let Some(cursor) = &query.cursor {
        params.push(("cursor", cursor.clone()));
    }
    let resp = send_with_retry(client, client.get(&url).query(&params))
        .await
        .with_context(|| format!("failed requesting reviews for app {app_id}"))?;
    let body: Value = resp.json().await.map_err(|e| {
        TypedError::new(
            ErrorKind::SourceChanged,
            format!("failed parsing reviews for app {app_id}: {e}"),
        )
    })?;
    if body.get("success").and_then(Value::as_i64) != Some(1) {
        return Err(TypedError::new(
            ErrorKind::NotFound,
            format!("no reviews available for app {app_id}"),
        )
        .into());
    }
    let s = body.get("query_summary").cloned().unwrap_or(Value::Null);
    let summary = ReviewSummary {
        score: i64_of(&s, "review_score"),
        label: str_of(&s, "review_score_desc"),
        total_positive: i64_of(&s, "total_positive"),
        total_negative: i64_of(&s, "total_negative"),
        total_reviews: i64_of(&s, "total_reviews"),
    };
    let reviews = body
        .get("reviews")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|r| {
                    let author = r.get("author").cloned().unwrap_or(Value::Null);
                    Review {
                        id: str_of(r, "recommendationid"),
                        author: str_of(&author, "personaname"),
                        steam_id: str_of(&author, "steamid"),
                        language: str_of(r, "language"),
                        voted_up: bool_of(r, "voted_up"),
                        votes_up: i64_of(r, "votes_up"),
                        votes_funny: i64_of(r, "votes_funny"),
                        playtime_hours: i64_of(&author, "playtime_forever") as f64 / 60.0,
                        playtime_at_review_hours: i64_of(&author, "playtime_at_review") as f64 / 60.0,
                        created: i64_of(r, "timestamp_created").max(0) as u64,
                        steam_purchase: bool_of(r, "steam_purchase"),
                        received_for_free: bool_of(r, "received_for_free"),
                        early_access: bool_of(r, "written_during_early_access"),
                        text: str_of(r, "review"),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    // Steam repeats the same cursor when the list is exhausted.
    let next_cursor = body
        .get("cursor")
        .and_then(Value::as_str)
        .map(String::from)
        .filter(|c| !c.is_empty() && query.cursor.as_deref() != Some(c.as_str()));
    Ok(ReviewPage {
        summary,
        reviews,
        next_cursor,
    })
}

/// One news post or announcement.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct NewsItem {
    pub gid: String,
    pub title: String,
    pub url: String,
    pub author: String,
    pub feed: String,
    pub date: u64,
    pub contents: String,
}

/// Fetch recent news for an app (no key needed).
pub async fn fetch_news(
    client: &reqwest::Client,
    app_id: u32,
    count: u32,
    max_length: u32,
) -> Result<Vec<NewsItem>> {
    let url = "https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/";
    let params = [
        ("appid", app_id.to_string()),
        ("count", count.to_string()),
        ("maxlength", max_length.to_string()),
        ("format", "json".to_string()),
        (
            "feeds",
            "steam_community_announcements,steam_community_events".to_string(),
        ),
    ];
    let resp = send_with_retry(client, client.get(url).query(&params))
        .await
        .with_context(|| format!("failed requesting news for app {app_id}"))?;
    let body: Value = resp.json().await.map_err(|e| {
        TypedError::new(
            ErrorKind::SourceChanged,
            format!("failed parsing news for app {app_id}: {e}"),
        )
    })?;
    let items = body
        .get("appnews")
        .and_then(|n| n.get("newsitems"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            TypedError::new(
                ErrorKind::SourceChanged,
                format!("unexpected news response shape for app {app_id}"),
            )
        })?;
    Ok(items
        .iter()
        .map(|n| NewsItem {
            gid: str_of(n, "gid"),
            title: str_of(n, "title"),
            url: str_of(n, "url"),
            author: str_of(n, "author"),
            feed: str_of(n, "feedlabel"),
            date: i64_of(n, "date").max(0) as u64,
            contents: crate::web::store::strip_html(&str_of(n, "contents")),
        })
        .collect())
}
