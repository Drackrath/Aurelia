//! Discover the user's Steam library from the Steam client's *own* on-disk data.
//!
//! `aurelia list` normally sources the full owned library from Steam's network
//! API (`fetch_owned_games`), which requires an `aurelia login`. On Linux the
//! Steam client is almost always already signed in and keeps the entire library
//! cached locally, so when Aurelia has no session (or is offline) we can still
//! surface every game by reading those caches directly:
//!
//! * `appcache/appinfo.vdf` — binary blob mapping every app id to its `common`
//!   metadata (`name`, `type`). Used to resolve names and to keep only games.
//! * `userdata/<id3>/config/localconfig.vdf` — text VDF listing the apps the
//!   signed-in user has local state for, including `Playtime` in minutes.
//! * `appcache/librarycache/<appid>/` — one directory per owned app (Steam
//!   pre-fetches artwork for owned titles). Broadens the set beyond apps that
//!   merely have local config.
//!
//! None of this requires network access or an Aurelia login. See
//! `docs/linux-library-discovery.md` for the full rationale.

use crate::core::config::detect_steam_path;
use crate::core::models::OwnedGame;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// SteamID64 of the first account in the public universe. Subtracting it from a
/// SteamID64 yields the 32-bit account id used for `userdata/<id>` directories.
const STEAMID64_BASE: u64 = 76_561_197_960_265_728;

/// Resolve the Steam *install* root — the directory that contains `appcache`
/// and `userdata` (not an arbitrary library folder). Mirrors the detection used
/// by the installed-game scanner but insists on a real `appcache` directory.
pub fn steam_install_root() -> Option<PathBuf> {
    detect_steam_path().filter(|p| p.join("appcache").is_dir())
}

/// Discover every owned *game* from the local Steam client caches.
///
/// Returns an empty vec (never errors) when the caches are missing or
/// unreadable, so callers can use it as a best-effort fallback.
pub async fn discover_local_owned_games() -> Vec<OwnedGame> {
    match steam_install_root() {
        Some(root) => discover_from_root(&root).await,
        None => Vec::new(),
    }
}

/// Same as [`discover_local_owned_games`] but against an explicit Steam root.
pub async fn discover_from_root(root: &Path) -> Vec<OwnedGame> {
    // Candidate app ids + playtime from the signed-in user's local config.
    let playtime = read_local_playtime(root).await;

    // Broaden the candidate set with every app that has a librarycache entry;
    // Steam pre-fetches artwork for owned apps that may lack local config.
    let mut candidates: HashSet<u32> = playtime.keys().copied().collect();
    candidates.extend(read_librarycache_appids(root).await);

    if candidates.is_empty() {
        return Vec::new();
    }

    // Resolve names/types for the candidates from appinfo.vdf.
    let appinfo_path = root.join("appcache").join("appinfo.vdf");
    let meta = match tokio::fs::read(&appinfo_path).await {
        Ok(bytes) => parse_appinfo(&bytes),
        Err(_) => HashMap::new(),
    };

    let mut games = Vec::new();
    for app_id in candidates {
        // Keep only titles Steam classifies as games (drops DLC, tools,
        // soundtracks, config apps, runtimes, …).
        let Some(info) = meta.get(&app_id).filter(|i| i.app_type.eq_ignore_ascii_case("game"))
        else {
            continue;
        };
        games.push(OwnedGame {
            app_id,
            name: info.name.clone(),
            playtime_forever_minutes: playtime.get(&app_id).copied().unwrap_or(0),
            local_manifest_ids: HashMap::new(),
            update_available: false,
        });
    }

    games.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
    games
}

/// Read `appid -> playtime (minutes)` from the most-recently-used account's
/// `localconfig.vdf`. App ids with no recorded playtime map to `0`.
async fn read_local_playtime(root: &Path) -> HashMap<u32, u32> {
    let Some(config_path) = locate_localconfig(root).await else {
        return HashMap::new();
    };
    match tokio::fs::read_to_string(&config_path).await {
        Ok(text) => parse_localconfig_apps(&text),
        Err(_) => HashMap::new(),
    }
}

/// Find the `localconfig.vdf` of the most-recently-used signed-in account,
/// falling back to any account that has one.
async fn locate_localconfig(root: &Path) -> Option<PathBuf> {
    let userdata = root.join("userdata");

    // Prefer the MostRecent user recorded in loginusers.vdf.
    if let Some(id3) = most_recent_account_id(root).await {
        let candidate = userdata.join(id3.to_string()).join("config/localconfig.vdf");
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    // Fallback: scan userdata for any localconfig.vdf.
    let mut dir = tokio::fs::read_dir(&userdata).await.ok()?;
    while let Ok(Some(entry)) = dir.next_entry().await {
        let candidate = entry.path().join("config/localconfig.vdf");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Resolve the 32-bit account id of the `MostRecent` user from
/// `config/loginusers.vdf`.
async fn most_recent_account_id(root: &Path) -> Option<u32> {
    let path = root.join("config/loginusers.vdf");
    let text = tokio::fs::read_to_string(&path).await.ok()?;

    // Walk the users block, tracking the SteamID64 of the entry whose
    // "MostRecent" is "1".
    let mut current_id64: Option<u64> = None;
    let mut fallback: Option<u64> = None;
    for line in text.lines() {
        let parts = quoted_tokens(line.trim());
        match parts.as_slice() {
            [id] if id.chars().all(|c| c.is_ascii_digit()) && id.len() >= 17 => {
                current_id64 = id.parse::<u64>().ok();
                if fallback.is_none() {
                    fallback = current_id64;
                }
            }
            [key, value] if key.eq_ignore_ascii_case("MostRecent") && value == "1" => {
                if let Some(id64) = current_id64 {
                    return account_id_from_id64(id64);
                }
            }
            _ => {}
        }
    }
    fallback.and_then(account_id_from_id64)
}

/// Convert a public-universe SteamID64 to its 32-bit account id (the value used
/// for `userdata/<id>` directories). `None` if the id predates the base.
fn account_id_from_id64(id64: u64) -> Option<u32> {
    id64.checked_sub(STEAMID64_BASE).map(|v| v as u32)
}

/// List app ids that have an `appcache/librarycache/<appid>` directory.
async fn read_librarycache_appids(root: &Path) -> HashSet<u32> {
    let mut ids = HashSet::new();
    let cache_dir = root.join("appcache").join("librarycache");
    if let Ok(mut dir) = tokio::fs::read_dir(&cache_dir).await {
        while let Ok(Some(entry)) = dir.next_entry().await {
            if let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            {
                ids.insert(id);
            }
        }
    }
    ids
}

/// Parse the `apps` blocks of a text `localconfig.vdf`, returning
/// `appid -> playtime minutes`. Every app id under an `apps` object is recorded
/// (playtime `0` when none is listed).
fn parse_localconfig_apps(text: &str) -> HashMap<u32, u32> {
    let mut result = HashMap::new();
    let mut path: Vec<String> = Vec::new();
    let mut pending_key: Option<String> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line == "{" {
            let key = pending_key.take().unwrap_or_default();
            // An app id is any direct child key of an "apps" object.
            if path.last().is_some_and(|p| p.eq_ignore_ascii_case("apps")) {
                if let Ok(id) = key.parse::<u32>() {
                    result.entry(id).or_insert(0);
                }
            }
            path.push(key);
            continue;
        }
        if line == "}" {
            path.pop();
            pending_key = None;
            continue;
        }

        let tokens = quoted_tokens(line);
        match tokens.as_slice() {
            [key] => pending_key = Some(key.clone()),
            [key, value] => {
                pending_key = None;
                // Inside an apps/<appid> object, capture Playtime in minutes.
                if key.eq_ignore_ascii_case("Playtime")
                    && path.len() >= 2
                    && path[path.len() - 2].eq_ignore_ascii_case("apps")
                {
                    if let (Ok(app_id), Ok(minutes)) =
                        (path[path.len() - 1].parse::<u32>(), value.parse::<u32>())
                    {
                        result.insert(app_id, minutes);
                    }
                }
            }
            _ => {}
        }
    }
    result
}

/// `common` metadata extracted from appinfo.vdf for a single app.
struct AppMeta {
    name: String,
    app_type: String,
}

/// Parse the binary `appinfo.vdf`, returning `appid -> AppMeta` for every app
/// that has a `common` block. Supports the v0x27/0x28/0x29 container layouts
/// (0x29 interns keys in a trailing string table).
///
/// Returns an empty map if the header is unrecognised; individual malformed
/// app records are skipped rather than aborting the whole parse.
fn parse_appinfo(bytes: &[u8]) -> HashMap<u32, AppMeta> {
    let mut out = HashMap::new();
    let mut pos = 0usize;

    let Some(magic) = read_u32(bytes, &mut pos) else {
        return out;
    };
    // Universe (ignored).
    if read_u32(bytes, &mut pos).is_none() {
        return out;
    }

    // v0x28 added a binary-VDF sha1 to each record; v0x29 additionally interns
    // keys in a string table referenced from the header.
    let (has_vdf_sha, string_table) = match magic {
        0x07564427 => (false, None),
        0x07564428 => (true, None),
        0x07564429 => {
            let Some(offset) = read_i64(bytes, &mut pos) else {
                return out;
            };
            (true, parse_string_table(bytes, offset as usize))
        }
        _ => return out,
    };

    // Per-record fixed header after appid+size: state(4) lastUpdated(4)
    // token(8) sha1(20) changeNumber(4) [+ vdf sha1(20)].
    let header_tail = 4 + 4 + 8 + 20 + 4 + if has_vdf_sha { 20 } else { 0 };

    loop {
        let Some(app_id) = read_u32(bytes, &mut pos) else {
            break;
        };
        if app_id == 0 {
            break; // sentinel terminating the app list
        }
        let Some(size) = read_u32(bytes, &mut pos) else {
            break;
        };
        let size = size as usize;
        // `size` counts from just after itself: fixed header tail + KV body.
        let record_start = pos;
        let record_end = record_start.saturating_add(size).min(bytes.len());
        let kv_start = record_start.saturating_add(header_tail);

        if kv_start <= record_end {
            if let Some(meta) =
                parse_app_common(&bytes[kv_start..record_end], string_table.as_deref())
            {
                out.insert(app_id, meta);
            }
        }

        // Advance to the next record using the declared size (robust to any
        // imperfection in KV parsing).
        pos = record_end;
    }

    out
}

/// Parse the trailing string table (v0x29): a u32 count followed by that many
/// NUL-terminated UTF-8 strings.
fn parse_string_table(bytes: &[u8], offset: usize) -> Option<Vec<String>> {
    let mut pos = offset;
    let count = read_u32(bytes, &mut pos)? as usize;
    let mut table = Vec::with_capacity(count.min(1 << 20));
    for _ in 0..count {
        table.push(read_cstr(bytes, &mut pos)?);
    }
    Some(table)
}

/// Walk one app's binary KV body and pull `common.name` / `common.type`.
fn parse_app_common(body: &[u8], strings: Option<&[String]>) -> Option<AppMeta> {
    let mut meta = AppMeta {
        name: String::new(),
        app_type: String::new(),
    };
    let mut pos = 0usize;
    walk_object(body, &mut pos, strings, false, &mut meta);
    if meta.name.is_empty() {
        // Without a name there is nothing useful to show.
        return None;
    }
    Some(meta)
}

/// Recursively walk a binary-VDF object, capturing `name`/`type` string fields
/// whenever we are directly inside a `common` object. Consumes through the
/// object's terminating `0x08` byte.
fn walk_object(
    buf: &[u8],
    pos: &mut usize,
    strings: Option<&[String]>,
    in_common: bool,
    meta: &mut AppMeta,
) {
    while *pos < buf.len() {
        let type_byte = buf[*pos];
        *pos += 1;
        match type_byte {
            0x08 => return, // end of object
            0x00 => {
                // Nested object.
                let Some(key) = read_key(buf, pos, strings) else {
                    return;
                };
                let nested_common = key.eq_ignore_ascii_case("common");
                walk_object(buf, pos, strings, nested_common, meta);
            }
            0x01 => {
                // String value.
                let Some(key) = read_key(buf, pos, strings) else {
                    return;
                };
                let Some(value) = read_cstr(buf, pos) else {
                    return;
                };
                if in_common {
                    if key.eq_ignore_ascii_case("name") {
                        meta.name = value;
                    } else if key.eq_ignore_ascii_case("type") {
                        meta.app_type = value;
                    }
                }
            }
            0x02 | 0x03 => {
                // int32 / float32 — keyed 4-byte scalar, value unused.
                if !skip_keyed_value(buf, pos, strings, 4) {
                    return;
                }
            }
            0x07 | 0x0b => {
                // uint64 / int64 — keyed 8-byte scalar, value unused.
                if !skip_keyed_value(buf, pos, strings, 8) {
                    return;
                }
            }
            _ => return, // unknown type; bail out of this app
        }
    }
}

/// Consume a keyed fixed-width scalar whose value is not needed: skip the key,
/// then advance `pos` past `value_len` value bytes. Returns `false` if the key
/// could not be read (caller should bail out of the object).
fn skip_keyed_value(
    buf: &[u8],
    pos: &mut usize,
    strings: Option<&[String]>,
    value_len: usize,
) -> bool {
    if read_key(buf, pos, strings).is_none() {
        return false;
    }
    *pos += value_len;
    true
}

/// Read a field key: a string-table index (v0x29) or an inline NUL-terminated
/// string (older formats).
fn read_key(buf: &[u8], pos: &mut usize, strings: Option<&[String]>) -> Option<String> {
    match strings {
        Some(table) => {
            let idx = read_u32(buf, pos)? as usize;
            table.get(idx).cloned()
        }
        None => read_cstr(buf, pos),
    }
}

fn read_u32(buf: &[u8], pos: &mut usize) -> Option<u32> {
    let end = pos.checked_add(4)?;
    let slice = buf.get(*pos..end)?;
    *pos = end;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
}

fn read_i64(buf: &[u8], pos: &mut usize) -> Option<i64> {
    let end = pos.checked_add(8)?;
    let slice = buf.get(*pos..end)?;
    *pos = end;
    Some(i64::from_le_bytes(slice.try_into().ok()?))
}

/// Read a NUL-terminated UTF-8 string (lossy), advancing past the terminator.
fn read_cstr(buf: &[u8], pos: &mut usize) -> Option<String> {
    let start = *pos;
    let rel = buf.get(start..)?.iter().position(|&b| b == 0)?;
    let s = String::from_utf8_lossy(&buf[start..start + rel]).into_owned();
    *pos = start + rel + 1;
    Some(s)
}

/// Split a VDF line into its double-quoted tokens (handles `"key"  "value"`).
fn quoted_tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_quote = false;
    let mut current = String::new();
    for ch in line.chars() {
        if ch == '"' {
            if in_quote {
                out.push(std::mem::take(&mut current));
            }
            in_quote = !in_quote;
            continue;
        }
        if in_quote {
            current.push(ch);
        }
    }
    out
}

#[cfg(test)]
#[path = "local_library_tests.rs"]
mod tests;
