//! User identifiers the CM can resolve without HTTP.
use crate::core::error::{ErrorKind, TypedError};
use regex::Regex;
use std::sync::LazyLock;

/// SteamID64 base for individual accounts.
pub const STEAMID64_INDIVIDUAL_BASE: u64 = 76_561_197_960_265_728;

static RE_PROFILE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"steamcommunity\.com/profiles/(\d{17})").unwrap());
static RE_VANITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"steamcommunity\.com/id/([^/?#\s]+)").unwrap());

/// What a user argument turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ident {
    /// The logged-in account.
    Me,
    /// A ready SteamID64.
    SteamId(u64),
    /// A persona name to match against the roster.
    Name(String),
}

/// Classify `input`; vanity URLs are rejected.
pub fn parse_ident(input: &str) -> Result<Ident, TypedError> {
    let q = input.trim();
    if q.is_empty() {
        return Err(TypedError::new(ErrorKind::InvalidInput, "empty user identifier"));
    }
    if q.eq_ignore_ascii_case("me") {
        return Ok(Ident::Me);
    }
    if let Some(c) = RE_PROFILE_ID.captures(q) {
        if let Ok(id) = c[1].parse::<u64>() {
            return Ok(Ident::SteamId(id));
        }
    }
    if let Some(c) = RE_VANITY.captures(q) {
        return Err(vanity_unsupported(&c[1]));
    }
    if let Ok(id) = q.parse::<u64>() {
        if id >= STEAMID64_INDIVIDUAL_BASE {
            return Ok(Ident::SteamId(id));
        }
        return Err(TypedError::new(
            ErrorKind::InvalidInput,
            format!("{id} is not a SteamID64 (expected a 17-digit id starting 7656…)"),
        ));
    }
    Ok(Ident::Name(q.trim_matches('/').to_string()))
}

fn vanity_unsupported(slug: &str) -> TypedError {
    TypedError::new(
        ErrorKind::InvalidInput,
        format!(
            "vanity URL `{slug}` cannot be resolved over the Steam connection; \
             pass the SteamID64, the /profiles/<id> URL, or a friend's name"
        ),
    )
}

/// Match a persona name or nickname against known friends.
pub fn match_friend_name<'a>(
    name: &str,
    friends: &'a [super::Friend],
) -> Result<&'a super::Friend, TypedError> {
    let needle = name.to_lowercase();
    let exact: Vec<&super::Friend> = friends
        .iter()
        .filter(|f| f.persona_name.as_deref().is_some_and(|n| n.to_lowercase() == needle))
        .collect();
    let candidates = if exact.is_empty() {
        friends
            .iter()
            .filter(|f| {
                f.persona_name
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase().starts_with(&needle))
            })
            .collect()
    } else {
        exact
    };
    match candidates.as_slice() {
        [one] => Ok(one),
        [] => Err(TypedError::new(
            ErrorKind::NotFound,
            format!("no friend named `{name}`; non-friends need a SteamID64 or /profiles/ URL"),
        )),
        many => {
            let names: Vec<String> = many
                .iter()
                .take(8)
                .map(|f| format!("{} ({})", f.persona_name.as_deref().unwrap_or("?"), f.steam_id))
                .collect();
            Err(TypedError::new(
                ErrorKind::InvalidInput,
                format!("`{name}` matches several friends: {}", names.join(", ")),
            ))
        }
    }
}

#[cfg(test)]
#[path = "ident_tests.rs"]
mod tests;
