//! Steam `CompatToolMapping` checks.
//!
//! Mappings store internal names, not folders.
//! Replaced tools orphan old mappings.

use std::path::{Path, PathBuf};

use serde::Serialize;
use steam_vdf_parser::Value;

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

#[cfg(test)]
#[path = "mapping_tests.rs"]
mod tests;
