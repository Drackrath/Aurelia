//! Steam `CompatToolMapping` checks.
//!
//! Mappings store internal names, not folders.
//! Replaced tools orphan old mappings.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use steam_vdf_parser::Value;

use crate::core::vdf_text::{Tok, vdf_tokens};

/// One `CompatToolMapping` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolMapping {
    /// App id; `0` means default.
    pub app_id: u64,
    /// Tool's internal name.
    pub tool: String,
    /// Whether Steam can resolve it.
    pub known: bool,
}

/// Linux runtimes Steam ships itself.
const VALVE_RUNTIMES: &[&str] = &[
    "steamlinuxruntime",
    "steamlinuxruntime_scout",
    "steamlinuxruntime_soldier",
    "steamlinuxruntime_sniper",
    "steamlinuxruntime_4",
    "legacysteamruntime",
    "legacy_steam_runtime",
    "scout",
    "scout_legacy",
    "soldier",
    "sniper",
    "steamrt4",
    "native",
];

/// Steam's own tools need no files.
pub fn is_valve_tool(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("proton_") || VALVE_RUNTIMES.contains(&lower.as_str())
}

/// Case-insensitive child lookup.
fn child<'a, 'b>(value: &'a Value<'b>, key: &str) -> Option<&'a Value<'b>> {
    value
        .as_obj()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v)
}

/// Tool names a `compatibilitytool.vdf` declares.
pub fn declared_tool_names(raw: &str) -> Vec<String> {
    let Ok(vdf) = steam_vdf_parser::parse_text(raw) else {
        return Vec::new();
    };
    child(vdf.value(), "compat_tools")
        .and_then(Value::as_obj)
        .map(|tools| tools.iter().map(|(k, _)| k.to_string()).collect())
        .unwrap_or_default()
}

/// `(app id, tool)` pairs from `config.vdf`.
pub fn mappings_from_config(raw: &str) -> Vec<(u64, String)> {
    let Ok(vdf) = steam_vdf_parser::parse_text(raw) else {
        return Vec::new();
    };
    let mut node = vdf.value();
    for key in ["Software", "Valve", "Steam", "CompatToolMapping"] {
        match child(node, key) {
            Some(next) => node = next,
            None => return Vec::new(),
        }
    }
    let Some(entries) = node.as_obj() else {
        return Vec::new();
    };
    let mut out: Vec<(u64, String)> = entries
        .iter()
        .filter_map(|(app, entry)| {
            let app_id = app.parse().ok()?;
            let tool = child(entry, "name")?.as_str()?.trim();
            // Empty name means no tool.
            (!tool.is_empty()).then(|| (app_id, tool.to_string()))
        })
        .collect();
    out.sort();
    out
}

/// Mark each mapping known or dangling.
pub fn classify(mappings: Vec<(u64, String)>, declared: &[String]) -> Vec<ToolMapping> {
    mappings
        .into_iter()
        .map(|(app_id, tool)| {
            let known = is_valve_tool(&tool) || declared.iter().any(|d| d == &tool);
            ToolMapping {
                app_id,
                tool,
                known,
            }
        })
        .collect()
}

/// Folders Steam scans for custom tools.
pub fn compat_tool_dirs(steam_root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![
        steam_root.join("compatibilitytools.d"),
        PathBuf::from("/usr/share/steam/compatibilitytools.d"),
        PathBuf::from("/usr/local/share/steam/compatibilitytools.d"),
    ];
    if let Some(extra) = std::env::var_os("STEAM_EXTRA_COMPAT_TOOLS_PATHS") {
        dirs.extend(std::env::split_paths(&extra));
    }
    dirs
}

/// Installed tools as `(name, folder)`.
pub fn installed_tools(steam_root: &Path) -> Vec<(String, String)> {
    let mut tools = Vec::new();
    for dir in compat_tool_dirs(steam_root) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let folder = entry.file_name().to_string_lossy().to_string();
            let vdf = entry.path().join("compatibilitytool.vdf");
            if let Ok(raw) = std::fs::read_to_string(&vdf) {
                for name in declared_tool_names(&raw) {
                    tools.push((name, folder.clone()));
                }
            }
        }
    }
    tools.sort();
    tools.dedup();
    tools
}

/// Every tool name declared on disk.
pub fn installed_tool_names(steam_root: &Path) -> Vec<String> {
    let mut names: Vec<String> = installed_tools(steam_root)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    names.dedup();
    names
}

/// The Steam root's mappings, classified.
pub fn steam_tool_mappings(steam_root: &Path) -> Vec<ToolMapping> {
    let config = steam_root.join("config").join("config.vdf");
    let raw = std::fs::read_to_string(config).unwrap_or_default();
    classify(
        mappings_from_config(&raw),
        &installed_tool_names(steam_root),
    )
}

/// A stable installed tool to suggest.
pub fn suggest_tool(tools: &[(String, String)]) -> String {
    // Auto-replaced "latest" folders break again.
    tools
        .iter()
        .filter(|(name, folder)| {
            !name.to_ascii_lowercase().contains("latest")
                && !folder.to_ascii_lowercase().contains("latest")
        })
        .map(|(name, _)| name)
        .max_by_key(|n| version_key(n))
        .cloned()
        .unwrap_or_else(|| "proton_experimental".to_string())
}

/// Numbers in a name, for ordering.
fn version_key(name: &str) -> Vec<u64> {
    name.split(|c: char| !c.is_ascii_digit())
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// Path to the mapping block.
const MAPPING_PATH: [&str; 4] = ["InstallConfigStore", "Software", "Valve", "Steam"];

/// Close index of block at `open`.
fn matching_close(toks: &[Tok<'_>], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, tok) in toks.iter().enumerate().skip(open) {
        match tok {
            Tok::Open(_) => depth += 1,
            Tok::Close(_) => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            Tok::Str(..) => {}
        }
    }
    None
}

/// Direct child block token indices.
fn child_block(toks: &[Tok<'_>], open: Option<usize>, key: &str) -> Option<(usize, usize, usize)> {
    let mut i = open.map_or(0, |o| o + 1);
    while i < toks.len() {
        match toks[i] {
            Tok::Close(_) => return None,
            Tok::Open(_) => i = matching_close(toks, i)? + 1,
            Tok::Str(_, k) => match toks.get(i + 1) {
                Some(Tok::Open(_)) => {
                    let close = matching_close(toks, i + 1)?;
                    if k.eq_ignore_ascii_case(key) {
                        return Some((i, i + 1, close));
                    }
                    i = close + 1;
                }
                Some(Tok::Str(..)) => i += 2,
                _ => i += 1,
            },
        }
    }
    None
}

/// Byte offset of a token.
fn tok_pos(tok: Tok<'_>) -> usize {
    match tok {
        Tok::Str(pos, _) | Tok::Open(pos) | Tok::Close(pos) => pos,
    }
}

/// Insert `text` before `close` line.
fn insert_before_close(raw: &mut String, close: usize, text: &str, nl: &str) {
    let line_start = raw[..close].rfind('\n').map_or(0, |n| n + 1);
    if raw[line_start..close].trim().is_empty() {
        raw.insert_str(line_start, text);
    } else {
        raw.insert_str(close, &format!("{nl}{text}"));
    }
}

/// One mapping entry, indented `depth` tabs.
fn mapping_entry(app_id: u32, tool: &str, depth: usize, nl: &str) -> String {
    let outer = "\t".repeat(depth);
    let inner = "\t".repeat(depth + 1);
    format!(
        "{outer}\"{app_id}\"{nl}{outer}{{{nl}\
         {inner}\"name\"\t\t\"{tool}\"{nl}\
         {inner}\"config\"\t\t\"\"{nl}\
         {inner}\"priority\"\t\t\"250\"{nl}\
         {outer}}}{nl}"
    )
}

/// Splice in one entry; `None` if mapped.
pub fn add_app_mapping(raw: &str, app_id: u32, tool: &str) -> Result<Option<String>> {
    if tool.is_empty() || tool.contains(['"', '{', '}', '\\', '\n', '\r']) {
        bail!("invalid compatibility tool name {tool:?}");
    }
    if steam_vdf_parser::parse_text(raw).is_err() {
        bail!("config.vdf is not valid VDF; left unchanged");
    }
    let before = mappings_from_config(raw);
    if before.iter().any(|(app, _)| *app == u64::from(app_id)) {
        return Ok(None);
    }

    let toks = vdf_tokens(raw);
    let mut open = None;
    for key in MAPPING_PATH {
        let (_, o, _) = child_block(&toks, open, key)
            .with_context(|| format!("config.vdf has no {} section", MAPPING_PATH.join("/")))?;
        open = Some(o);
    }
    let steam_open = open.unwrap_or_default();
    let nl = if raw.contains("\r\n") { "\r\n" } else { "\n" };
    let depth = MAPPING_PATH.len() + 1;
    let mut out = raw.to_string();
    match child_block(&toks, Some(steam_open), "CompatToolMapping") {
        Some((_, map_open, map_close)) => {
            let entry = mapping_entry(app_id, tool, depth, nl);
            match child_block(&toks, Some(map_open), &app_id.to_string()) {
                // Empty name: replace that entry.
                Some((key, _, close)) => {
                    let start = tok_pos(toks[key]);
                    let end = tok_pos(toks[close]) + 1;
                    out.replace_range(start..end, entry.trim_start_matches('\t').trim_end());
                }
                None => insert_before_close(&mut out, tok_pos(toks[map_close]), &entry, nl),
            }
        }
        None => {
            let steam_close =
                matching_close(&toks, steam_open).context("config.vdf is truncated")?;
            let tabs = "\t".repeat(depth - 1);
            let block = format!(
                "{tabs}\"CompatToolMapping\"{nl}{tabs}{{{nl}{}{tabs}}}{nl}",
                mapping_entry(app_id, tool, depth, nl)
            );
            insert_before_close(&mut out, tok_pos(toks[steam_close]), &block, nl);
        }
    }

    // Read back before trusting it.
    let mut expected = before;
    expected.push((u64::from(app_id), tool.to_string()));
    expected.sort();
    if steam_vdf_parser::parse_text(&out).is_err() || mappings_from_config(&out) != expected {
        bail!("config.vdf edit did not verify; left unchanged");
    }
    Ok(Some(out))
}

/// Outcome of [`ensure_app_mapping`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappingWrite {
    /// New entry written.
    Added,
    /// App already had this tool.
    Existing(String),
}

/// Map `app_id` to `tool`; Steam closed.
pub fn ensure_app_mapping(steam_root: &Path, app_id: u32, tool: &str) -> Result<MappingWrite> {
    let path = steam_root.join("config").join("config.vdf");
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed reading {}", path.display()))?;
    let Some(updated) = add_app_mapping(&raw, app_id, tool)? else {
        let existing = mappings_from_config(&raw)
            .into_iter()
            .find(|(app, _)| *app == u64::from(app_id))
            .map(|(_, name)| name)
            .unwrap_or_default();
        return Ok(MappingWrite::Existing(existing));
    };
    replace_config(&path, &updated)?;
    Ok(MappingWrite::Added)
}

/// Back up, then replace atomically.
fn replace_config(path: &Path, text: &str) -> Result<()> {
    let backup = path.with_file_name("config.vdf.aurelia-backup");
    std::fs::copy(path, &backup)
        .with_context(|| format!("failed backing up {}", path.display()))?;
    crate::core::atomic::write_atomic(path, text.as_bytes())
}

/// Splice out one entry; `None` if absent.
pub fn remove_app_mapping(raw: &str, app_id: u32) -> Result<Option<String>> {
    if steam_vdf_parser::parse_text(raw).is_err() {
        bail!("config.vdf is not valid VDF; left unchanged");
    }
    let toks = vdf_tokens(raw);
    let mut open = None;
    for key in MAPPING_PATH.iter().copied().chain(["CompatToolMapping"]) {
        let Some((_, o, _)) = child_block(&toks, open, key) else {
            return Ok(None);
        };
        open = Some(o);
    }
    let Some((key, _, close)) = child_block(&toks, open, &app_id.to_string()) else {
        return Ok(None);
    };
    let key_pos = tok_pos(toks[key]);
    let close_pos = tok_pos(toks[close]);
    // Whole lines when the entry owns them.
    let line_start = raw[..key_pos].rfind('\n').map_or(0, |n| n + 1);
    let start = if raw[line_start..key_pos].trim().is_empty() {
        line_start
    } else {
        key_pos
    };
    let line_end = raw[close_pos..].find('\n').map(|n| close_pos + n + 1);
    let end = match line_end {
        Some(end) if raw[close_pos + 1..end].trim().is_empty() => end,
        _ => close_pos + 1,
    };
    let mut out = raw.to_string();
    out.replace_range(start..end, "");

    // Read back before trusting it.
    let expected: Vec<(u64, String)> = mappings_from_config(raw)
        .into_iter()
        .filter(|(app, _)| *app != u64::from(app_id))
        .collect();
    if steam_vdf_parser::parse_text(&out).is_err() || mappings_from_config(&out) != expected {
        bail!("config.vdf edit did not verify; left unchanged");
    }
    Ok(Some(out))
}

/// Drop `app_id`'s mapping; Steam closed.
pub fn remove_app_mapping_file(steam_root: &Path, app_id: u32) -> Result<bool> {
    let path = steam_root.join("config").join("config.vdf");
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed reading {}", path.display()))?;
    let Some(updated) = remove_app_mapping(&raw, app_id)? else {
        return Ok(false);
    };
    replace_config(&path, &updated)?;
    Ok(true)
}

/// Tools and whether they run Windows builds.
pub fn declared_tool_platforms(raw: &str) -> Vec<(String, bool)> {
    let Ok(vdf) = steam_vdf_parser::parse_text(raw) else {
        return Vec::new();
    };
    let Some(tools) = child(vdf.value(), "compat_tools").and_then(Value::as_obj) else {
        return Vec::new();
    };
    tools
        .iter()
        .map(|(name, tool)| {
            let windows = child(tool, "from_oslist")
                .and_then(Value::as_str)
                .is_some_and(|os| os.to_ascii_lowercase().contains("windows"));
            (name.to_string(), windows)
        })
        .collect()
}

/// Installed tools with their platform.
pub fn installed_tool_platforms(steam_root: &Path) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for dir in compat_tool_dirs(steam_root) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let vdf = entry.path().join("compatibilitytool.vdf");
            if let Ok(raw) = std::fs::read_to_string(&vdf) {
                out.extend(declared_tool_platforms(&raw));
            }
        }
    }
    out
}

/// Whether `tool` runs Windows builds.
pub fn runs_windows_builds(tool: &str, declared: &[(String, bool)]) -> bool {
    if let Some((_, windows)) = declared.iter().find(|(name, _)| name == tool) {
        return *windows;
    }
    // Undeclared: judge by name.
    let lower = tool.to_ascii_lowercase();
    !VALVE_RUNTIMES.contains(&lower.as_str()) && lower.contains("proton")
}

#[cfg(test)]
mod tests;
