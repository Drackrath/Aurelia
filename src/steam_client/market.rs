//! Steam Community Market, inventory, and wallet — read-only (Phase 1).
//!
//! These are **web** endpoints (`steamcommunity.com`), so they need a logged-in web
//! session rather than the CM connection. [`SteamClient::web_session`] mints that
//! session (Phase 0): it exchanges the CM session's refresh token for a short-lived
//! **web access token** via `Authentication.GenerateAccessTokenForApp`, then builds
//! the `steamLoginSecure`/`sessionid` cookies (see [`crate::web_session`]).
//!
//! Public lookups (item price, market search) need no session and are free functions;
//! account-scoped reads (inventory, wallet, my listings) are `SteamClient` methods.
use super::*;
use crate::web_session::{WebSession, ECON_IMAGE_BASE};
use serde_json::Value;
use std::sync::LazyLock;
use steam_vent_proto::steammessages_auth_steamclient::{
    CAuthentication_AccessToken_GenerateForApp_Request,
    CAuthentication_AccessToken_GenerateForApp_Response,
};

/// One item in an account inventory.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InventoryItem {
    /// Unique asset id (per item instance).
    pub asset_id: String,
    /// Economy class id (shared by identical items).
    pub class_id: String,
    /// Display name.
    pub name: String,
    /// Canonical market name used by the market endpoints.
    pub market_hash_name: String,
    /// Item type/category text (e.g. "Trading Card").
    pub item_type: String,
    /// Stack size for this asset.
    pub amount: u64,
    /// Whether the item can be traded.
    pub tradable: bool,
    /// Whether the item can be sold on the Community Market.
    pub marketable: bool,
    /// Full icon URL, if the item has one.
    pub icon_url: Option<String>,
}

/// Market price summary for an item (`priceoverview`). Prices are Steam-formatted
/// currency strings.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MarketPrice {
    pub market_hash_name: String,
    pub lowest_price: Option<String>,
    pub median_price: Option<String>,
    /// Number of items sold in the last 24h.
    pub volume: Option<String>,
}

/// One result from a market search.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MarketSearchResult {
    pub name: String,
    pub market_hash_name: String,
    pub app_id: u32,
    pub app_name: String,
    /// Number of active sell listings.
    pub sell_listings: u32,
    /// Lowest sell price, in the currency's minor units (cents).
    pub sell_price: u64,
    /// Pre-formatted lowest sell price.
    pub sell_price_text: String,
}

/// One of the account's active sell listings.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MyListing {
    pub listing_id: String,
    pub market_hash_name: String,
    /// Listed price in the currency's minor units (cents), as reported by Steam.
    pub price: u64,
}

/// One of the account's open buy orders.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MyBuyOrder {
    pub buy_order_id: String,
    pub market_hash_name: String,
    /// Per-item price in the currency's minor units (cents).
    pub price: u64,
    /// Quantity still wanted.
    pub quantity: u64,
}

/// The account's market listings and open buy orders.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MyMarketState {
    pub listings: Vec<MyListing>,
    pub buy_orders: Vec<MyBuyOrder>,
}

/// The account's Steam Wallet balance.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WalletBalance {
    /// Balance in the currency's minor units (cents).
    pub balance_cents: i64,
    /// Steam currency id.
    pub currency: u32,
    /// Wallet country code.
    pub country: String,
    /// Human-readable balance (e.g. "12.34 EUR").
    pub formatted: String,
}

impl SteamClient {
    /// **Phase 0:** mint a Steam web session from the current CM session.
    ///
    /// Exchanges the session's refresh token (held by the live connection as its
    /// "access token") for a fresh, web-scoped access token via
    /// `Authentication.GenerateAccessTokenForApp`, then builds the cookie set. This
    /// reuses the one shared connection — no extra logon.
    pub async fn web_session(&self) -> Result<WebSession> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;
        let steam_id = u64::from(connection.steam_id());
        let refresh_token = connection
            .access_token()
            .context("no Steam token available for a web session; run `aurelia login`")?
            .to_string();

        let mut req = CAuthentication_AccessToken_GenerateForApp_Request::new();
        req.set_refresh_token(refresh_token);
        req.set_steamid(steam_id);

        let resp: CAuthentication_AccessToken_GenerateForApp_Response = connection
            .service_method(req)
            .await
            .map_err(|e| anyhow!("failed to mint a web access token: {e}"))?;
        let token = resp.access_token();
        if token.is_empty() {
            bail!("Steam returned an empty web access token");
        }
        WebSession::new(steam_id, token, connection.ip_country_code().as_deref())
    }

    /// List the logged-in account's inventory for an app. `context_id` is the
    /// inventory context (usually 2; e.g. Steam community items use 6).
    pub async fn inventory(&self, app_id: u32, context_id: u32) -> Result<Vec<InventoryItem>> {
        let web = self.web_session().await?;
        let steam_id = web.steam_id();
        let url = format!(
            "https://steamcommunity.com/inventory/{steam_id}/{app_id}/{context_id}?l=english&count=2000"
        );
        parse_inventory(&web.get_text(&url).await?)
    }

    /// Fetch the account's active market listings and open buy orders.
    pub async fn my_market_listings(&self) -> Result<MyMarketState> {
        let web = self.web_session().await?;
        let body = web
            .get_text("https://steamcommunity.com/market/mylistings/?norender=1&count=100")
            .await?;
        parse_my_listings(&body)
    }

    /// Fetch the account's Steam Wallet balance.
    pub async fn wallet(&self) -> Result<WalletBalance> {
        let web = self.web_session().await?;
        let body = web.get_text("https://steamcommunity.com/market/").await?;
        parse_wallet(&body)
    }
}

/// A `reqwest` client for public (no-auth) market lookups.
fn public_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("aurelia")
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("failed to build HTTP client")
}

/// Look up an item's market price (`priceoverview`). Public — no session required.
/// `currency` is the Steam currency id (1 = USD, 2 = GBP, 3 = EUR, …).
pub async fn market_price(
    app_id: u32,
    market_hash_name: &str,
    currency: u32,
) -> Result<MarketPrice> {
    let resp = public_client()?
        .get("https://steamcommunity.com/market/priceoverview/")
        .query(&[
            ("appid", app_id.to_string()),
            ("currency", currency.to_string()),
            ("market_hash_name", market_hash_name.to_string()),
        ])
        .send()
        .await
        .context("market price request failed")?;
    if resp.status().as_u16() == 429 {
        bail!("Steam is rate-limiting price lookups (HTTP 429); try again in a few minutes");
    }
    let v: Value = resp.json().await.context("invalid price-overview response")?;
    if v.get("success").and_then(Value::as_bool) != Some(true) {
        bail!(
            "no market data for '{market_hash_name}' (it may not be marketable, or the name/app id \
             is wrong — names are case-sensitive and exact)"
        );
    }
    Ok(MarketPrice {
        market_hash_name: market_hash_name.to_string(),
        lowest_price: v.get("lowest_price").and_then(Value::as_str).map(String::from),
        median_price: v.get("median_price").and_then(Value::as_str).map(String::from),
        volume: v.get("volume").and_then(Value::as_str).map(String::from),
    })
}

/// Search the Community Market. Public — no session required. Returns
/// `(total_count, page_of_results)`.
pub async fn market_search(
    query: Option<&str>,
    app_id: Option<u32>,
    count: u32,
) -> Result<(u32, Vec<MarketSearchResult>)> {
    let mut params: Vec<(&str, String)> = vec![
        ("norender", "1".to_string()),
        ("start", "0".to_string()),
        ("count", count.to_string()),
    ];
    if let Some(q) = query {
        params.push(("query", q.to_string()));
    }
    if let Some(a) = app_id {
        params.push(("appid", a.to_string()));
    }

    let resp = public_client()?
        .get("https://steamcommunity.com/market/search/render/")
        .query(&params)
        .send()
        .await
        .context("market search request failed")?;
    if resp.status().as_u16() == 429 {
        bail!("Steam is rate-limiting market search (HTTP 429); try again in a few minutes");
    }
    let v: Value = resp.json().await.context("invalid market-search response")?;
    let total = v.get("total_count").and_then(Value::as_u64).unwrap_or(0) as u32;
    let results = v
        .get("results")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_search_result).collect())
        .unwrap_or_default();
    Ok((total, results))
}

fn parse_search_result(v: &Value) -> Option<MarketSearchResult> {
    Some(MarketSearchResult {
        name: v.get("name")?.as_str()?.to_string(),
        market_hash_name: v.get("hash_name").and_then(Value::as_str).unwrap_or_default().to_string(),
        app_id: v
            .get("asset_description")
            .and_then(|a| a.get("appid"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        app_name: v.get("app_name").and_then(Value::as_str).unwrap_or_default().to_string(),
        sell_listings: v.get("sell_listings").and_then(Value::as_u64).unwrap_or(0) as u32,
        sell_price: v.get("sell_price").and_then(Value::as_u64).unwrap_or(0),
        sell_price_text: v.get("sell_price_text").and_then(Value::as_str).unwrap_or_default().to_string(),
    })
}

/// Coerce a JSON value that may be a number or a numeric string into i64.
fn as_i64_loose(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// Coerce a JSON value that may be a number or numeric string into u64.
fn as_u64_loose(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn parse_inventory(body: &str) -> Result<Vec<InventoryItem>> {
    let v: Value = serde_json::from_str(body).context(
        "could not parse the inventory response (the web session may be invalid — try \
         `aurelia login --reconnect`)",
    )?;
    // An explicit failure (e.g. private inventory) comes back as success:0 + error.
    if v.get("success").and_then(Value::as_i64) == Some(0) {
        let err = v.get("error").and_then(Value::as_str).unwrap_or("inventory unavailable");
        bail!("{err}");
    }
    let Some(assets) = v.get("assets").and_then(Value::as_array) else {
        return Ok(Vec::new()); // empty inventory
    };

    // Index descriptions by (classid, instanceid) so each asset can be enriched.
    let mut descriptions: HashMap<(String, String), &Value> = HashMap::new();
    if let Some(arr) = v.get("descriptions").and_then(Value::as_array) {
        for d in arr {
            let cid = d.get("classid").and_then(Value::as_str).unwrap_or_default().to_string();
            let iid = d.get("instanceid").and_then(Value::as_str).unwrap_or("0").to_string();
            descriptions.insert((cid, iid), d);
        }
    }

    let mut items = Vec::with_capacity(assets.len());
    for a in assets {
        let class_id = a.get("classid").and_then(Value::as_str).unwrap_or_default().to_string();
        let instance_id = a.get("instanceid").and_then(Value::as_str).unwrap_or("0").to_string();
        let asset_id = a.get("assetid").and_then(Value::as_str).unwrap_or_default().to_string();
        let amount = a.get("amount").and_then(as_u64_loose).unwrap_or(1);

        let desc = descriptions.get(&(class_id.clone(), instance_id));
        let get = |k: &str| desc.and_then(|d| d.get(k).and_then(Value::as_str)).unwrap_or_default();
        let flag = |k: &str| desc.and_then(|d| d.get(k).and_then(Value::as_i64)).unwrap_or(0) == 1;
        let icon = desc
            .and_then(|d| d.get("icon_url").and_then(Value::as_str))
            .filter(|s| !s.is_empty())
            .map(|s| format!("{ECON_IMAGE_BASE}{s}"));

        items.push(InventoryItem {
            asset_id,
            class_id,
            name: get("name").to_string(),
            market_hash_name: get("market_hash_name").to_string(),
            item_type: get("type").to_string(),
            amount,
            tradable: flag("tradable"),
            marketable: flag("marketable"),
            icon_url: icon,
        });
    }
    Ok(items)
}

fn parse_my_listings(body: &str) -> Result<MyMarketState> {
    let v: Value = serde_json::from_str(body).context(
        "could not parse your market listings (the web session may be invalid — try \
         `aurelia login --reconnect`)",
    )?;
    // `success:false` means the request wasn't authenticated (vs. an authed account
    // that simply has no listings, which returns `success:true` + empty arrays).
    if matches!(v.get("success").and_then(Value::as_bool), Some(false))
        || matches!(v.get("success").and_then(Value::as_i64), Some(0))
    {
        bail!(
            "the market session was rejected (no authenticated response). Run \
             `aurelia login --reconnect`."
        );
    }
    let listings = v
        .get("listings")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_my_listing).collect())
        .unwrap_or_default();
    let buy_orders = v
        .get("buy_orders")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_my_buy_order).collect())
        .unwrap_or_default();
    Ok(MyMarketState { listings, buy_orders })
}

fn parse_my_listing(v: &Value) -> Option<MyListing> {
    let listing_id = v.get("listingid")?.as_str()?.to_string();
    let asset = v.get("asset");
    let name = asset
        .and_then(|a| {
            a.get("market_hash_name")
                .or_else(|| a.get("name"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default()
        .to_string();
    let price = v.get("price").and_then(as_u64_loose).unwrap_or(0);
    Some(MyListing { listing_id, market_hash_name: name, price })
}

fn parse_my_buy_order(v: &Value) -> Option<MyBuyOrder> {
    let buy_order_id = v.get("buy_orderid")?.as_str()?.to_string();
    Some(MyBuyOrder {
        buy_order_id,
        market_hash_name: v.get("hash_name").and_then(Value::as_str).unwrap_or_default().to_string(),
        price: v.get("price").and_then(as_u64_loose).unwrap_or(0),
        quantity: v.get("quantity").and_then(as_u64_loose).unwrap_or(0),
    })
}

fn parse_wallet(html: &str) -> Result<WalletBalance> {
    // The market page embeds `g_rgWalletInfo = {…};` as an inline script var.
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"g_rgWalletInfo\s*=\s*(\{.*?\})\s*;").unwrap());
    let caps = RE.captures(html).context(
        "could not read your wallet (no `g_rgWalletInfo` on the market page). Make sure you are \
         logged in and the account is market-eligible.",
    )?;
    let json: Value =
        serde_json::from_str(&caps[1]).context("failed to parse wallet info")?;

    let currency = json.get("wallet_currency").and_then(Value::as_u64).unwrap_or(0) as u32;
    let country = json.get("wallet_country").and_then(Value::as_str).unwrap_or_default().to_string();
    let balance_cents = json.get("wallet_balance").and_then(as_i64_loose).unwrap_or(0);

    Ok(WalletBalance {
        formatted: format_currency(balance_cents, currency),
        balance_cents,
        currency,
        country,
    })
}

/// Format a minor-unit (cents) amount with the currency's code. Falls back to the
/// numeric currency id for unmapped currencies.
pub fn format_currency(cents: i64, currency: u32) -> String {
    let code = currency_code(currency);
    format!("{:.2} {code}", cents as f64 / 100.0)
}

/// Map a Steam currency id to its ISO-ish code (common subset).
fn currency_code(currency: u32) -> String {
    match currency {
        1 => "USD",
        2 => "GBP",
        3 => "EUR",
        4 => "CHF",
        5 => "RUB",
        6 => "PLN",
        7 => "BRL",
        8 => "JPY",
        9 => "NOK",
        13 => "SGD",
        16 => "KRW",
        17 => "TRY",
        18 => "UAH",
        19 => "MXN",
        20 => "CAD",
        21 => "AUD",
        22 => "NZD",
        23 => "CNY",
        24 => "INR",
        28 => "ZAR",
        29 => "HKD",
        _ => return format!("(currency {currency})"),
    }
    .to_string()
}
