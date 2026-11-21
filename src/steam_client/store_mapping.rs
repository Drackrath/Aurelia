//! Store item to app-info mapping.

use super::*;
use crate::core::time::unix_to_ymd;

/// Human-facing store metadata for an app, sourced from the `StoreBrowse.GetItems`
/// service method over the Steam CM connection (no HTTPS storefront API). This is
/// the protocol-native replacement for the fields Aurelia previously scraped from
/// `store.steampowered.com/api/appdetails`.
///
/// Note: a few storefront-only fields have no equivalent in this protocol and are
/// intentionally absent — system requirements, Metacritic score, and
/// name-resolved store tags/genres/categories (StoreBrowse returns only numeric
/// ids for those, which would need a separate localized tag dictionary).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StoreAppInfo {
    pub app_id: u32,
    pub name: String,
    pub app_type: String,
    pub is_free: bool,
    pub is_early_access: bool,
    pub short_description: String,
    pub full_description: String,
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    pub franchises: Vec<String>,
    pub release_date: Option<String>,
    pub coming_soon: bool,
    pub price: Option<String>,
    pub discount_pct: i32,
    pub platforms: Vec<String>,
    pub review_summary: Option<String>,
    /// Artwork URLs (header/cover/hero/background/logo).
    pub assets: StoreAppAssets,
    /// Country the prices were quoted for.
    #[serde(default)]
    pub country: String,
    /// Final price in minor units.
    #[serde(default)]
    pub price_cents: Option<i64>,
    /// Pre-discount price, formatted.
    #[serde(default)]
    pub original_price: Option<String>,
    /// Pre-discount price in minor units.
    #[serde(default)]
    pub original_price_cents: Option<i64>,
    /// Unix time the active discount ends.
    #[serde(default)]
    pub discount_end: Option<u64>,
    /// Store refuses sale in this country.
    #[serde(default)]
    pub region_locked: bool,
    /// Every package and bundle the store offers.
    #[serde(default)]
    pub purchase_options: Vec<StorePurchaseOption>,
    /// Store tags, heaviest first.
    #[serde(default)]
    pub tags: Vec<StoreTag>,
    /// Age rating from the store, if any.
    #[serde(default)]
    pub rating: Option<StoreRating>,
    /// Official social links.
    #[serde(default)]
    pub links: Vec<StoreLink>,
    /// Screenshot URLs, all-ages first.
    #[serde(default)]
    pub screenshots: Vec<String>,
    /// Trailers, highlights first.
    #[serde(default)]
    pub trailers: Vec<StoreTrailer>,
}

/// A store tag with its per-app weight.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StoreTag {
    pub id: u32,
    pub name: Option<String>,
    pub weight: u32,
}

/// Age rating (e.g. PEGI, ESRB).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StoreRating {
    pub system: String,
    pub rating: String,
    pub descriptors: Vec<String>,
    pub required_age: i32,
}

/// An official link (youtube, discord, …).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StoreLink {
    pub kind: String,
    pub url: String,
}

/// A store trailer; video URL rarely present.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StoreTrailer {
    pub name: String,
    pub movie_id: Option<u64>,
    pub url: Option<String>,
    pub thumbnail: Option<String>,
}

/// A package or bundle for an app.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StorePurchaseOption {
    /// `package` or `bundle`.
    pub kind: String,
    pub id: u32,
    pub name: String,
    pub price: Option<String>,
    pub price_cents: Option<i64>,
    pub original_price: Option<String>,
    pub discount_pct: i32,
    /// Extra saving from bundling, if a bundle.
    pub bundle_discount_pct: i32,
    pub included_games: u32,
    pub discount_end: Option<u64>,
}

pub(super) fn purchase_option_summary(
    p: &steam_vent_proto::steammessages_storebrowse_steamclient::store_item::PurchaseOption,
) -> StorePurchaseOption {
    let (kind, id) = if p.bundleid() > 0 {
        ("bundle", p.bundleid())
    } else {
        ("package", p.packageid())
    };
    let nonempty = |s: &str| (!s.is_empty()).then(|| s.to_string());
    StorePurchaseOption {
        kind: kind.to_string(),
        id: id.max(0) as u32,
        name: p.purchase_option_name().to_string(),
        price: nonempty(p.formatted_final_price()),
        price_cents: (p.final_price_in_cents() > 0).then(|| p.final_price_in_cents()),
        original_price: nonempty(p.formatted_original_price())
            .filter(|_| p.original_price_in_cents() > p.final_price_in_cents()),
        discount_pct: p.discount_pct(),
        bundle_discount_pct: p.bundle_discount_pct(),
        included_games: p.included_game_count().max(0) as u32,
        discount_end: p
            .active_discounts
            .iter()
            .map(|d| d.discount_end_date())
            .max()
            .filter(|&t| t > 0)
            .map(u64::from),
    }
}

/// Artwork URLs for a store app. Built from the StoreBrowse `assets` block when
/// present, falling back to Steam's conventional per-appid CDN paths so a caller
/// (e.g. Heroic) always has usable URLs instead of guessing them itself.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StoreAppAssets {
    /// Wide store header / capsule (`header.jpg`).
    pub header: Option<String>,
    /// Portrait library cover (`library_600x900`).
    pub capsule: Option<String>,
    /// Wide library hero background.
    pub hero: Option<String>,
    /// Store-page background.
    pub background: Option<String>,
    /// Transparent game logo (`logo.png`). Not in StoreBrowse — always the
    /// conventional CDN URL.
    pub logo: Option<String>,
}

/// Collect the non-empty display names from a list of `CreatorHomeLink`s
/// (developers / publishers / franchises in a `StoreItem.basic_info`).
pub(super) fn creator_names(
    creators: &[steam_vent_proto::steammessages_storebrowse_steamclient::store_item::basic_info::CreatorHomeLink],
) -> Vec<String> {
    creators
        .iter()
        .map(|c| c.name().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Map a `StoreBrowse` `StoreItem` protobuf into our display-oriented
/// [`StoreAppInfo`]. Defensive throughout: every nested message is optional, so
/// missing data yields empty fields rather than failing.
pub(super) fn store_item_to_app_info(item: &StoreItem, country: &str) -> StoreAppInfo {
    let basic = item.basic_info.as_ref();

    let release = item.release.as_ref();
    let release_date = release.and_then(|r| {
        let custom = r.custom_release_date_message();
        if !custom.is_empty() {
            Some(custom.to_string())
        } else if r.steam_release_date() > 0 {
            Some(unix_to_ymd(r.steam_release_date() as i64))
        } else {
            None
        }
    });

    let platforms = item
        .platforms
        .as_ref()
        .map(|p| {
            let mut v = Vec::new();
            if p.windows() {
                v.push("Windows".to_string());
            }
            if p.mac() {
                v.push("macOS".to_string());
            }
            if p.steamos_linux() {
                v.push("Linux".to_string());
            }
            v
        })
        .unwrap_or_default();

    let option = item.best_purchase_option.as_ref();
    let (price, discount_pct) = match option {
        _ if item.is_free() => (Some("Free".to_string()), 0),
        Some(p) if !p.formatted_final_price().is_empty() => (
            Some(p.formatted_final_price().to_string()),
            p.discount_pct(),
        ),
        _ => (None, 0),
    };
    let priced = option.filter(|_| !item.is_free() && price.is_some());
    let price_cents = priced.map(|p| p.final_price_in_cents());
    let original_price_cents = priced
        .map(|p| p.original_price_in_cents())
        .filter(|&c| c > 0 && Some(c) != price_cents);
    let original_price = priced
        .map(|p| p.formatted_original_price().to_string())
        .filter(|s| !s.is_empty() && original_price_cents.is_some());
    let discount_end = priced
        .and_then(|p| {
            p.active_discounts
                .iter()
                .map(|d| d.discount_end_date())
                .max()
        })
        .filter(|&t| t > 0)
        .map(u64::from);

    let review_summary = item
        .reviews
        .as_ref()
        .and_then(|r| r.summary_filtered.as_ref())
        .and_then(|s| {
            let label = s.review_score_label();
            if label.is_empty() {
                None
            } else {
                Some(format!(
                    "{} ({}% positive, {} reviews)",
                    label,
                    s.percent_positive(),
                    s.review_count()
                ))
            }
        });

    StoreAppInfo {
        app_id: item.appid(),
        name: item.name().to_string(),
        app_type: store_app_type_label(item.type_()),
        is_free: item.is_free(),
        is_early_access: item.is_early_access(),
        short_description: basic
            .map(|b| b.short_description().to_string())
            .unwrap_or_default(),
        full_description: crate::web::store::strip_html(item.full_description()),
        developers: basic
            .map(|b| creator_names(&b.developers))
            .unwrap_or_default(),
        publishers: basic
            .map(|b| creator_names(&b.publishers))
            .unwrap_or_default(),
        franchises: basic
            .map(|b| creator_names(&b.franchises))
            .unwrap_or_default(),
        release_date,
        coming_soon: release.map(|r| r.is_coming_soon()).unwrap_or(false),
        price,
        discount_pct,
        platforms,
        review_summary,
        assets: store_assets(item),
        country: country.to_string(),
        price_cents,
        original_price,
        original_price_cents,
        discount_end,
        region_locked: item.unvailable_for_country_restriction(),
        purchase_options: item
            .purchase_options
            .iter()
            .map(purchase_option_summary)
            .collect(),
        tags: store_tags(item),
        rating: store_rating(item),
        links: store_links(item),
        screenshots: store_screenshots(item),
        trailers: store_trailers(item),
    }
}

/// Tags sorted by weight, names unresolved.
pub(super) fn store_tags(item: &StoreItem) -> Vec<StoreTag> {
    let mut tags: Vec<StoreTag> = item
        .tags
        .iter()
        .filter(|t| t.tagid() != 0)
        .map(|t| StoreTag {
            id: t.tagid(),
            name: None,
            weight: t.weight(),
        })
        .collect();
    tags.sort_by(|a, b| b.weight.cmp(&a.weight).then(a.id.cmp(&b.id)));
    tags
}

pub(super) fn store_rating(item: &StoreItem) -> Option<StoreRating> {
    let r = item.game_rating.as_ref()?;
    if r.rating().is_empty() && r.type_().is_empty() {
        return None;
    }
    // Board name parsed from the image path.
    let system = if r.type_().is_empty() {
        r.image_url()
            .split("game_ratings/")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or("")
            .to_string()
    } else {
        r.type_().to_string()
    };
    Some(StoreRating {
        system,
        rating: r.rating().to_string(),
        descriptors: r.descriptors.clone(),
        required_age: r.required_age(),
    })
}

pub(super) fn store_links(item: &StoreItem) -> Vec<StoreLink> {
    use steam_vent_proto::steammessages_storebrowse_steamclient::EStoreLinkType as L;
    item.links
        .iter()
        .filter(|l| !l.url().is_empty())
        .map(|l| {
            let kind = match l.link_type.and_then(|t| t.enum_value().ok()) {
                Some(L::k_EStoreLinkType_YouTube) => "youtube",
                Some(L::k_EStoreLinkType_Facebook) => "facebook",
                Some(L::k_EStoreLinkType_Twitter) => "twitter",
                Some(L::k_EStoreLinkType_Twitch) => "twitch",
                Some(L::k_EStoreLinkType_Discord) => "discord",
                _ => "other",
            };
            StoreLink {
                kind: kind.to_string(),
                url: l.url().to_string(),
            }
        })
        .collect()
}

/// Expand `${FILENAME}` in a StoreBrowse URL template.
pub(super) fn asset_url(fmt: &str, filename: &str) -> Option<String> {
    if fmt.is_empty() || filename.is_empty() {
        return None;
    }
    let path = fmt.replace("${FILENAME}", filename);
    Some(if path.starts_with("http") {
        path
    } else {
        format!("https://shared.cloudflare.steamstatic.com/store_item_assets/{path}")
    })
}

pub(super) fn store_screenshots(item: &StoreItem) -> Vec<String> {
    let fmt = item
        .assets
        .as_ref()
        .map(|a| a.asset_url_format())
        .unwrap_or("");
    let Some(shots) = item.screenshots.as_ref() else {
        return Vec::new();
    };
    shots
        .all_ages_screenshots
        .iter()
        .chain(shots.mature_content_screenshots.iter())
        .filter_map(|s| screenshot_url(fmt, s.filename()))
        .collect()
}

/// Screenshot filenames may already be full paths.
pub(super) fn screenshot_url(fmt: &str, filename: &str) -> Option<String> {
    if filename.starts_with("steam/") {
        return Some(format!(
            "https://shared.cloudflare.steamstatic.com/store_item_assets/{filename}"
        ));
    }
    asset_url(fmt, filename)
}

pub(super) fn store_trailers(item: &StoreItem) -> Vec<StoreTrailer> {
    let Some(trailers) = item.trailers.as_ref() else {
        return Vec::new();
    };
    trailers
        .highlights
        .iter()
        .chain(trailers.other_trailers.iter())
        .map(|t| {
            // Prefer the mp4 rendition at max quality.
            let source = t
                .trailer_max
                .iter()
                .chain(t.trailer_480p.iter())
                .find(|s| s.type_().contains("mp4"))
                .or_else(|| t.trailer_max.first())
                .or_else(|| t.trailer_480p.first());
            // Thumbnail path is `<movie id>/<hash>/movie_600x337.jpg`.
            let movie_id = t
                .screenshot_medium()
                .split('/')
                .next()
                .and_then(|s| s.parse::<u64>().ok());
            StoreTrailer {
                name: t.trailer_name().to_string(),
                movie_id,
                url: source.and_then(|s| asset_url(t.trailer_url_format(), s.filename())),
                thumbnail: asset_url(t.trailer_url_format(), t.screenshot_medium()),
            }
        })
        .collect()
}

/// Resolve a `StoreItem`'s artwork URLs. Each is built from the StoreBrowse
/// `assets` block (`asset_url_format` with the asset filename substituted for
/// `${FILENAME}`) when available, otherwise from Steam's conventional per-appid
/// CDN paths so the caller always gets a usable URL.
pub(super) fn store_assets(item: &StoreItem) -> StoreAppAssets {
    let appid = item.appid();
    let assets = item.assets.as_ref();
    let fmt = assets.map(|a| a.asset_url_format()).unwrap_or("");

    let from_fmt = |filename: &str| -> Option<String> {
        if filename.is_empty() || fmt.is_empty() {
            return None;
        }
        let path = fmt.replace("${FILENAME}", filename);
        if path.starts_with("http") {
            Some(path)
        } else {
            Some(format!(
                "https://shared.cloudflare.steamstatic.com/store_item_assets/{path}"
            ))
        }
    };
    let cdn =
        |file: &str| format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/{file}");
    let pick = |native: Option<String>, fallback: &str| native.or_else(|| Some(cdn(fallback)));

    StoreAppAssets {
        header: pick(assets.and_then(|a| from_fmt(a.header())), "header.jpg"),
        capsule: pick(
            assets.and_then(|a| from_fmt(a.library_capsule_2x())),
            "library_600x900_2x.jpg",
        ),
        hero: pick(
            assets.and_then(|a| from_fmt(a.library_hero_2x())),
            "library_hero.jpg",
        ),
        background: pick(
            assets.and_then(|a| from_fmt(a.page_background())),
            "page_bg_generated_v6b.jpg",
        ),
        // StoreBrowse has no logo field; use the conventional CDN URL.
        logo: Some(cdn("logo.png")),
    }
}

/// Human-readable label for a `StoreBrowse` app type.
pub(super) fn store_app_type_label(t: EStoreAppType) -> String {
    match t {
        EStoreAppType::k_EStoreAppType_Game => "Game",
        EStoreAppType::k_EStoreAppType_Demo => "Demo",
        EStoreAppType::k_EStoreAppType_Mod => "Mod",
        EStoreAppType::k_EStoreAppType_Movie => "Movie",
        EStoreAppType::k_EStoreAppType_DLC => "DLC",
        EStoreAppType::k_EStoreAppType_Guide => "Guide",
        EStoreAppType::k_EStoreAppType_Software => "Software",
        EStoreAppType::k_EStoreAppType_Video => "Video",
        EStoreAppType::k_EStoreAppType_Series => "Series",
        EStoreAppType::k_EStoreAppType_Episode => "Episode",
        EStoreAppType::k_EStoreAppType_Hardware => "Hardware",
        EStoreAppType::k_EStoreAppType_Music => "Soundtrack",
        EStoreAppType::k_EStoreAppType_Beta => "Beta",
        EStoreAppType::k_EStoreAppType_Tool => "Tool",
        EStoreAppType::k_EStoreAppType_Advertising => "Advertising",
    }
    .to_string()
}
