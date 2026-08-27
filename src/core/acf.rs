//! Single-pass parsing of Steam appmanifest (`.acf`) text.
//!
//! Every ACF field reader in the codebase goes through [`parse_app_manifest`]
//! so the format quirks (the `UserConfig` nesting of `BetaKey`, first-match
//! semantics, `0` meaning "no owner") live in exactly one place.

use crate::core::utils::extract_quoted_values;

/// Top-level fields of an `appmanifest_<appid>.acf`, extracted in one pass.
#[derive(Debug, Default)]
pub struct AppManifest {
    pub app_id: Option<u32>,
    pub install_dir: Option<String>,
    /// Raw `name` value (may be empty); see [`AppManifest::display_name`].
    pub name: Option<String>,
    /// `LastOwner` SteamID64; `None` when absent or `0`.
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

    /// The `name` value trimmed, `None` when absent or empty.
    pub fn display_name(&self) -> Option<String> {
        let name = self.name.as_deref()?.trim();
        (!name.is_empty()).then(|| name.to_string())
    }
}

/// Parse the top-level scalar fields of an appmanifest. The `BetaKey` of the
/// nested `UserConfig` block is the only non-top-level value read.
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
            "installdir" if m.install_dir.is_none() => m.install_dir = Some(value.to_string()),
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
