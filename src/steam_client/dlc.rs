//! DLC lists in appmanifests and PICS.

use super::*;

/// Collect the DLC appids whose content is recorded as installed in an appmanifest.
/// Steam tags each DLC depot under `InstalledDepots` with a `"dlcappid" "<id>"` line.
pub(super) fn parse_installed_dlc_appids(content: &str) -> HashSet<u32> {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#""dlcappid"\s*"(\d+)""#).unwrap());
    RE.captures_iter(content)
        .filter_map(|caps| caps[1].parse::<u32>().ok())
        .collect()
}

/// Collect the DLC appids listed in an appmanifest's `DisabledDLC` value(s).
/// The value is a comma-separated list of appids; multiple blocks may each carry one.
pub(super) fn parse_disabled_dlc_appids(content: &str) -> HashSet<u32> {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#""DisabledDLC"\s*"([^"]*)""#).unwrap());
    RE.captures_iter(content)
        .flat_map(|caps| {
            caps[1]
                .split(',')
                .filter_map(|s| s.trim().parse::<u32>().ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Add or remove a DLC appid from every `DisabledDLC` list in an appmanifest's text.
/// When `disabled` is true and no `DisabledDLC` key exists, one is inserted into the
/// `UserConfig` and `MountedConfig` blocks.
pub(super) fn apply_dlc_disabled(content: &str, dlc_appid: u32, disabled: bool) -> String {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#""DisabledDLC"(\s*)"([^"]*)""#).unwrap());
    let dlc = dlc_appid.to_string();

    if RE.is_match(content) {
        return RE
            .replace_all(content, |caps: &regex::Captures| {
                let mut list: Vec<String> = caps[2]
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty() && *s != dlc)
                    .collect();
                if disabled {
                    list.push(dlc.clone());
                }
                format!("\"DisabledDLC\"{}\"{}\"", &caps[1], list.join(","))
            })
            .into_owned();
    }

    if disabled {
        // No DisabledDLC key yet — add one to the per-user config blocks.
        let mut out = content.to_string();
        for block in ["MountedConfig", "UserConfig"] {
            if let Some(pos) = out.find(&format!("\"{block}\""))
                && let Some(rel) = out[pos..].find('{')
            {
                let at = pos + rel + 1;
                out.insert_str(at, &format!("\n\t\t\"DisabledDLC\"\t\t\"{dlc}\""));
            }
        }
        return out;
    }

    content.to_string()
}

/// DLC app ids declared by an app's PICS section, read from the canonical
/// `extended/listofdlc` field (a comma-separated string of app ids). Order is
/// preserved and duplicates dropped.
pub(crate) fn dlc_ids_from_section(section: &steam_vdf_parser::Value) -> Vec<u32> {
    let mut dlcs: Vec<u32> = Vec::new();
    if let Some(list) = section.get_str(&["extended", "listofdlc"]) {
        for id in list.split(',').filter_map(|p| p.trim().parse::<u32>().ok()) {
            if !dlcs.contains(&id) {
                dlcs.push(id);
            }
        }
    }
    dlcs
}
