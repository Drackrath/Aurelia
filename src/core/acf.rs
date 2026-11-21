//! Single-pass Steam appmanifest parsing.

use std::collections::HashMap;

use crate::core::utils::extract_quoted_values;

/// Top-level appmanifest fields, one pass.
#[derive(Debug, Default)]
pub struct AppManifest {
    pub app_id: Option<u32>,
    pub install_dir: Option<String>,
    /// Raw `name`; see [`AppManifest::display_name`].
    pub name: Option<String>,
    /// `LastOwner`; `None` when absent/zero.
    pub last_owner: Option<u64>,
    pub state_flags: Option<u32>,
    pub last_updated: u64,
    pub build_id: u64,
    /// `UserConfig/BetaKey`, defaulting to `public`.
    pub active_branch: String,
}

impl AppManifest {
    /// `StateFullyInstalled` (4) present in `StateFlags`.
    pub fn fully_installed(&self) -> bool {
        self.state_flags.is_some_and(|flags| flags & 4 != 0)
    }

    /// `StateUpdateRequired` (2) present in `StateFlags`.
    pub fn update_pending(&self) -> bool {
        self.state_flags.is_some_and(|flags| flags & 2 != 0)
    }

    /// Trimmed `name`, `None` when empty.
    pub fn display_name(&self) -> Option<String> {
        let name = self.name.as_deref()?.trim();
        (!name.is_empty()).then(|| name.to_string())
    }
}

/// Parse top-level fields plus `BetaKey`.
pub fn parse_app_manifest(raw: &str) -> AppManifest {
    let mut m = AppManifest {
        active_branch: "public".to_string(),
        ..Default::default()
    };
    let mut in_user_config = false;
    let mut branch_set = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        let parts = extract_quoted_values(trimmed);

        if parts.len() == 1 && parts[0].eq_ignore_ascii_case("userconfig") {
            in_user_config = true;
            continue;
        }

        if trimmed == "{" || trimmed == "}" {
            if trimmed == "}" && in_user_config {
                in_user_config = false;
            }
            continue;
        }

        if parts.len() < 2 {
            continue;
        }
        let value = &parts[1];

        if in_user_config {
            if !branch_set && parts[0].eq_ignore_ascii_case("betakey") && !value.trim().is_empty() {
                m.active_branch = value.to_string();
                branch_set = true;
            }
            continue;
        }

        match parts[0].to_ascii_lowercase().as_str() {
            "appid" if m.app_id.is_none() => m.app_id = value.parse().ok(),
            "installdir" if m.install_dir.is_none() => {
                // Refuse names escaping steamapps/common.
                if crate::core::utils::is_safe_dir_name(value) {
                    m.install_dir = Some(value.to_string());
                } else {
                    tracing::warn!("ignoring unsafe appmanifest installdir {value:?}");
                }
            }
            "name" if m.name.is_none() => m.name = Some(value.to_string()),
            // "0" means no owner recorded; treat as unknown.
            "lastowner" if m.last_owner.is_none() => {
                m.last_owner = value.parse().ok().filter(|&id| id != 0)
            }
            "stateflags" if m.state_flags.is_none() => m.state_flags = value.parse().ok(),
            "lastupdated" => m.last_updated = value.parse().unwrap_or(0),
            "buildid" => m.build_id = value.parse().unwrap_or(0),
            _ => {}
        }
    }

    m
}

/// `InstalledDepots` depot to manifest map.
pub fn installed_depots(raw: &str) -> HashMap<u64, u64> {
    let mut manifests = HashMap::new();
    let mut in_installed_depots = false;
    let mut current_depot: Option<u64> = None;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.contains("\"InstalledDepots\"") {
            in_installed_depots = true;
            continue;
        }

        if !in_installed_depots {
            continue;
        }

        if trimmed == "}" {
            if current_depot.is_some() {
                current_depot = None;
                continue;
            }
            break;
        }

        let quoted = extract_quoted_values(trimmed);
        if quoted.len() == 1 {
            if let Ok(depot_id) = quoted[0].parse::<u64>() {
                current_depot = Some(depot_id);
            }
        } else if quoted.len() >= 2
            && quoted[0] == "manifest"
            && let (Some(depot), Ok(manifest)) = (current_depot, quoted[1].parse::<u64>())
        {
            manifests.insert(depot, manifest);
        }
    }

    manifests
}

/// Set `UserConfig/BetaKey` to `branch`.
pub fn rewrite_branch(raw: &str, branch: &str) -> String {
    let mut out = Vec::new();
    let mut in_user_config = false;
    let mut branch_updated = false;

    for line in raw.lines() {
        let trimmed = line.trim();

        if trimmed.eq_ignore_ascii_case("\"UserConfig\"") {
            in_user_config = true;
            out.push(line.to_string());
            continue;
        }

        if in_user_config && trimmed == "{" {
            out.push(line.to_string());
            continue;
        }

        if in_user_config && trimmed == "}" {
            if !branch_updated {
                out.push(format!("\t\t\"BetaKey\"\t\t\"{branch}\""));
            }
            in_user_config = false;
            out.push(line.to_string());
            continue;
        }

        if in_user_config {
            let quoted = extract_quoted_values(trimmed);
            if !quoted.is_empty() && quoted[0].eq_ignore_ascii_case("betakey") {
                let indent = line
                    .chars()
                    .take_while(|ch| ch.is_whitespace())
                    .collect::<String>();
                out.push(format!("{indent}\"BetaKey\"\t\t\"{branch}\""));
                branch_updated = true;
                continue;
            }
        }

        out.push(line.to_string());
    }

    // Assumes Steam wrote a UserConfig.

    out.join("\n")
}

#[cfg(test)]
#[path = "acf_tests.rs"]
mod tests;
