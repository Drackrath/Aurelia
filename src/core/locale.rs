//! Locale detection for Steam language and country.

/// Parsed POSIX / BCP-47 locale tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemLocale {
    pub language: String,
    pub script: Option<String>,
    pub region: Option<String>,
}

/// Env vars consulted, most specific first.
const LOCALE_VARS: [&str; 4] = ["LC_ALL", "LC_MESSAGES", "LC_MONETARY", "LANG"];

/// Latin-American regions that use Steam's `latam`.
const LATAM_REGIONS: [&str; 19] = [
    "MX", "AR", "CL", "CO", "PE", "VE", "EC", "GT", "CU", "BO", "DO", "HN", "PY", "SV", "NI", "CR",
    "PA", "UY", "PR",
];

/// Parse `ll[_RR][.enc][@mod]` or `ll-Script-RR`.
pub fn parse_locale(raw: &str) -> Option<SystemLocale> {
    let tag = raw.trim().split(['.', '@']).next()?.trim();
    if tag.is_empty() || tag.eq_ignore_ascii_case("C") || tag.eq_ignore_ascii_case("POSIX") {
        return None;
    }
    let mut parts = tag.split(['_', '-']);
    let language = parts.next()?.to_ascii_lowercase();
    if !(2..=3).contains(&language.len()) || !language.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut script = None;
    let mut region = None;
    for part in parts {
        let alpha = part.chars().all(|c| c.is_ascii_alphabetic());
        match part.len() {
            4 if alpha && script.is_none() => {
                let mut chars = part.chars();
                let head = chars.next().map(|c| c.to_ascii_uppercase()).unwrap_or_default();
                script = Some(format!("{head}{}", chars.as_str().to_ascii_lowercase()));
            }
            2 if alpha && region.is_none() => region = Some(part.to_ascii_uppercase()),
            _ => {}
        }
    }
    Some(SystemLocale {
        language,
        script,
        region,
    })
}

/// First locale env var that parses.
pub fn detect_env_locale() -> Option<SystemLocale> {
    LOCALE_VARS
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find_map(|value| parse_locale(&value))
}

fn is_traditional_chinese(locale: &SystemLocale) -> bool {
    locale.script.as_deref() == Some("Hant")
        || matches!(locale.region.as_deref(), Some("TW" | "HK" | "MO"))
}

/// Steam API language name for a locale.
pub fn steam_language(locale: &SystemLocale) -> &'static str {
    let region = locale.region.as_deref();
    match locale.language.as_str() {
        "en" => "english",
        "de" => "german",
        "fr" => "french",
        "it" => "italian",
        "es" if region.is_some_and(|r| LATAM_REGIONS.contains(&r)) => "latam",
        "es" => "spanish",
        "pt" if region == Some("BR") => "brazilian",
        "pt" => "portuguese",
        "zh" if is_traditional_chinese(locale) => "tchinese",
        "zh" => "schinese",
        "ja" => "japanese",
        "ko" => "koreana",
        "ru" => "russian",
        "pl" => "polish",
        "nl" => "dutch",
        "sv" => "swedish",
        "da" => "danish",
        "fi" => "finnish",
        "no" | "nb" | "nn" => "norwegian",
        "cs" => "czech",
        "hu" => "hungarian",
        "ro" => "romanian",
        "bg" => "bulgarian",
        "el" => "greek",
        "tr" => "turkish",
        "uk" => "ukrainian",
        "th" => "thai",
        "vi" => "vietnamese",
        "id" => "indonesian",
        _ => "english",
    }
}

/// Store country for a locale; language default otherwise.
pub fn steam_country(locale: &SystemLocale) -> String {
    if let Some(cc) = locale.region.as_deref().and_then(normalize_country) {
        return cc;
    }
    match locale.language.as_str() {
        "zh" if is_traditional_chinese(locale) => "TW",
        "zh" => "CN",
        "de" => "DE",
        "fr" => "FR",
        "it" => "IT",
        "es" => "ES",
        "pt" => "PT",
        "ja" => "JP",
        "ko" => "KR",
        "ru" => "RU",
        "pl" => "PL",
        "nl" => "NL",
        "sv" => "SE",
        "da" => "DK",
        "fi" => "FI",
        "no" | "nb" | "nn" => "NO",
        "cs" => "CZ",
        "hu" => "HU",
        "ro" => "RO",
        "bg" => "BG",
        "el" => "GR",
        "tr" => "TR",
        "uk" => "UA",
        "th" => "TH",
        "vi" => "VN",
        "id" => "ID",
        _ => "US",
    }
    .to_string()
}

/// Uppercase two-letter code; `UK` becomes `GB`.
pub fn normalize_country(raw: &str) -> Option<String> {
    let code = raw.trim().to_ascii_uppercase();
    if code.len() != 2 || !code.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(if code == "UK" { "GB".to_string() } else { code })
}

#[cfg(test)]
#[path = "locale_tests.rs"]
mod tests;
