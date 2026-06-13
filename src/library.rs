use crate::config::{detect_steam_path, load_launcher_config};
use crate::models::{GameLibrary, GameModel, LibraryGame, LocalGame, OwnedGame};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;

/// App ids that appear as `appmanifest_*.acf` files (and in the owned-games
/// list) but are not actual games: Steam runtimes, redistributables, Proton,
/// server tools, etc. These are hidden from the library so they don't show up
/// as launchable titles. Mirrors Heroic's `ignoredSteamAppIds`.
pub const IGNORED_STEAM_APP_IDS: &[u32] = &[
    228980,  // Steamworks Common Redistributables
    1070560, // Steam Linux Runtime 1.0 (scout)
    1391110, // Steam Linux Runtime 2.0 (soldier)
    1628350, // Steam Linux Runtime 3.0 (sniper)
    1493710, // Proton Experimental
    2348590, // Proton 8.0
];

/// Games whose name starts with any of these prefixes are Steam tooling rather
/// than user games and are hidden from the library. Catches Proton/runtime
/// builds whose app ids aren't in [`IGNORED_STEAM_APP_IDS`]. Mirrors Heroic's
/// `ignoredSteamAppNamePrefixes`.
pub const IGNORED_STEAM_APP_NAME_PREFIXES: &[&str] = &[
    "Steam Linux Runtime",
    "Proton",
    "Steamworks Common Redistributables",
];

/// Whether an app is Steam tooling (runtime/redistributable/Proton/server tool)
/// rather than a user-facing game, and so should be hidden from the library.
pub fn is_ignored_steam_app(app_id: u32, name: &str) -> bool {
    if IGNORED_STEAM_APP_IDS.contains(&app_id) {
        return true;
    }
    let name = name.trim_start();
    IGNORED_STEAM_APP_NAME_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

#[derive(Debug, Deserialize)]
struct LibraryFoldersFile {
    #[serde(default)]
    libraryfolders: HashMap<String, LibraryFolderRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LibraryFolderRecord {
    LegacyPath(String),
    Detailed {
        path: Option<String>,
        #[serde(flatten)]
        _other: HashMap<String, serde_json::Value>,
    },
    Ignore(#[allow(dead_code)] HashMap<String, serde_json::Value>),
}


#[derive(Debug, Clone)]
pub struct InstalledAppInfo {
    pub install_path: PathBuf,
    pub active_branch: String,
    pub name: Option<String>,
    /// SteamID64 of the account that owns this local install (`LastOwner` in the
    /// appmanifest). Differs from the logged-in user for Family-Shared games.
    pub last_owner: Option<u64>,
}

pub async fn find_local_games() -> Result<Vec<LocalGame>> {
    let installed_info = scan_installed_app_info().await?;
    Ok(installed_info
        .into_iter()
        .map(|(app_id, info)| LocalGame {
            app_id,
            name: info.name.unwrap_or_else(|| format!("App {app_id}")),
            install_dir: info.install_path,
            proton_version: None,
            active_branch: info.active_branch,
        })
        .collect())
}

pub async fn scan_installed_app_info() -> Result<HashMap<u32, InstalledAppInfo>> {
    let config = load_launcher_config().await.ok();
    let config_path = config.as_ref().and_then(|cfg| {
        let p = PathBuf::from(&cfg.steam_library_path);
        (p.join("steamapps").exists() || p.join("Steam").join("steamapps").exists()).then_some(p)
    });

    let root = config_path
        .or_else(detect_steam_path)
        .unwrap_or_else(default_steam_root);
    tracing::debug!("scanning library root: {:?}", root);
    let mut installed = scan_library_info(&root).await?;

    if config.is_some_and(|cfg| cfg.windows_steam_discovery_enabled) {
        let master_steam = crate::utils::get_master_steam_config();
        if master_steam.wine_prefix.exists() {
            tracing::debug!("scanning Windows Steam root: {:?}", master_steam.wine_prefix);
            // Windows Steam layout is drive_c/Program Files (x86)/Steam
            let windows_steam_root = master_steam.wine_prefix.join("drive_c/Program Files (x86)/Steam");
            if windows_steam_root.exists() {
                let windows_installed = scan_library_info(&windows_steam_root).await.unwrap_or_default();
                for (app_id, info) in windows_installed {
                    // Prefer native/standard Linux Steam if duplicate
                    installed.entry(app_id).or_insert(info);
                }
            }
        }
    }

    Ok(installed)
}

pub async fn scan_installed_app_paths() -> Result<HashMap<u32, String>> {
    let info_map = scan_installed_app_info().await?;
    Ok(info_map
        .into_iter()
        .map(|(appid, info)| (appid, info.install_path.to_string_lossy().to_string()))
        .collect())
}

pub async fn scan_installed_app_paths_pathbuf() -> Result<HashMap<u32, PathBuf>> {
    let info_map = scan_installed_app_info().await?;
    Ok(info_map
        .into_iter()
        .map(|(appid, info)| (appid, info.install_path))
        .collect())
}

pub async fn scan_library_info(root_path: &Path) -> Result<HashMap<u32, InstalledAppInfo>> {
    let mut installed = HashMap::new();
    let mut libraries = vec![root_path.to_path_buf()];

    let library_folders_path = root_path.join("steamapps").join("libraryfolders.vdf");
    let extra_libraries = parse_library_folders(library_folders_path)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("could not parse libraryfolders.vdf: {}", e);
            Vec::new()
        });
    libraries.extend(extra_libraries);

    // Steam doesn't always register every library in libraryfolders.vdf (and the file
    // may live on a different root than the one we're scanning). Probe all connected
    // drives for Steam library folders so games on e.g. F:\SteamLibrary are found.
    libraries.extend(discover_drive_libraries());

    libraries.sort();
    libraries.dedup();

    for library_root in libraries {
        let steamapps = library_root.join("steamapps");
        if !steamapps.exists() {
            continue;
        }

        let mut dir = fs::read_dir(&steamapps)
            .await
            .with_context(|| format!("failed to read {}", steamapps.display()))?;

        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if !is_app_manifest(&path) {
                continue;
            }

            match parse_app_manifest_info(&path).await {
                Ok(Some((app_id, info))) => {
                    installed.insert(app_id, info);
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("skipping bad manifest {:?}: {}", path, e),
            }
        }
    }

    Ok(installed)
}

/// Probe every connected drive for Steam library folders.
///
/// Steam libraries are frequently placed at locations that may be missing from
/// `libraryfolders.vdf` (or on a drive whose `.vdf` we never read), e.g.
/// `F:\SteamLibrary`. We scan each drive root for well-known library folder
/// names and keep those that contain a `steamapps` directory.
pub fn discover_drive_libraries() -> Vec<PathBuf> {
    let mut found = Vec::new();

    #[cfg(target_os = "windows")]
    {
        // Common Steam library folder names, relative to a drive root.
        const CANDIDATES: &[&str] = &[
            "SteamLibrary",
            "Steam",
            "SteamGames",
            "Games\\SteamLibrary",
            "Program Files (x86)\\Steam",
            "Program Files\\Steam",
        ];

        for letter in b'A'..=b'Z' {
            let drive = PathBuf::from(format!("{}:\\", letter as char));
            if !drive.exists() {
                continue;
            }
            for candidate in CANDIDATES {
                let library = drive.join(candidate);
                if library.join("steamapps").is_dir() {
                    found.push(library);
                }
            }
        }
    }

    found.sort();
    found.dedup();
    found
}

/// Collect every Steam library root we can discover: the configured library,
/// the auto-detected install, the platform default, anything referenced by a
/// `libraryfolders.vdf`, and a probe of all connected drives.
pub async fn all_library_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    if let Ok(cfg) = load_launcher_config().await {
        let p = PathBuf::from(&cfg.steam_library_path);
        if !p.as_os_str().is_empty() {
            roots.push(p);
        }
    }
    if let Some(detected) = detect_steam_path() {
        roots.push(detected);
    }
    roots.push(default_steam_root());

    // Expand each root via its libraryfolders.vdf.
    let mut extra = Vec::new();
    for root in &roots {
        let vdf = root.join("steamapps").join("libraryfolders.vdf");
        if let Ok(found) = parse_library_folders(vdf).await {
            extra.extend(found);
        }
    }
    roots.extend(extra);
    roots.extend(discover_drive_libraries());

    roots.sort();
    roots.dedup();
    roots
}

fn default_steam_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(program_files_x86) = std::env::var("PROGRAMFILES(X86)") {
            return PathBuf::from(program_files_x86).join("Steam");
        }
        if let Ok(program_files) = std::env::var("PROGRAMFILES") {
            return PathBuf::from(program_files).join("Steam");
        }
        return PathBuf::from(r"C:\Program Files (x86)\Steam");
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Some(detected) = detect_steam_path() {
            return detected;
        }
        directories::BaseDirs::new()
            .map(|d| d.home_dir().to_path_buf())
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "~".to_string()))
            })
            .join(".steam/steam")
    }
}

fn is_app_manifest(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };

    name.starts_with("appmanifest_") && name.ends_with(".acf")
}

pub async fn parse_library_folders(path: PathBuf) -> Result<Vec<PathBuf>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw = fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed reading {}", path.display()))?;

    let parsed = keyvalues_serde::from_str::<LibraryFoldersFile>(&raw)
        .context("failed to parse libraryfolders.vdf with keyvalues-serde")?;

    let mut libraries = Vec::new();
    for (key, value) in parsed.libraryfolders {
        if !key.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }

        match value {
            LibraryFolderRecord::LegacyPath(p) if !p.is_empty() => libraries.push(PathBuf::from(p)),
            LibraryFolderRecord::Detailed { path: Some(p), .. } if !p.is_empty() => {
                libraries.push(PathBuf::from(p))
            }
            _ => {}
        }
    }

    libraries.sort();
    libraries.dedup();
    Ok(libraries)
}

async fn parse_app_manifest_info(path: &Path) -> Result<Option<(u32, InstalledAppInfo)>> {
    let raw = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed reading {}", path.display()))?;

    let mut app_id = None;
    let mut install_dir_name = None;
    let mut name = None;
    let mut last_owner = None;
    let mut active_branch = "public".to_string();

    let mut in_user_config = false;

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

        if parts.len() >= 2 {
            let key = parts[0].to_lowercase();
            let value = &parts[1];

            if !in_user_config {
                match key.as_str() {
                    "appid" => app_id = value.parse::<u32>().ok(),
                    "installdir" => install_dir_name = Some(value.to_string()),
                    "name" => name = Some(value.to_string()),
                    // "0" means no owner recorded; treat as unknown.
                    "lastowner" => last_owner = value.parse::<u64>().ok().filter(|&id| id != 0),
                    _ => {}
                }
            } else if key == "betakey" && !value.trim().is_empty() {
                active_branch = value.to_string();
            }
        }
    }

    let (Some(id), Some(dir)) = (app_id, install_dir_name) else {
        return Ok(None);
    };
    let install_path = path
        .parent()
        .map(|p| p.join("common").join(dir))
        .unwrap_or_default();
    Ok(Some((
        id,
        InstalledAppInfo {
            install_path,
            active_branch,
            name,
            last_owner,
        },
    )))
}

fn extract_quoted_values(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_quote = false;
    let mut current = String::new();
    for ch in line.chars() {
        if ch == '"' {
            if in_quote {
                out.push(current.clone());
                current.clear();
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

pub fn build_game_library(
    owned: Vec<OwnedGame>,
    installed_info: HashMap<u32, InstalledAppInfo>,
    steam_id: Option<u64>,
) -> GameLibrary {
    let mut games = Vec::new();
    // App ids already emitted from the owned list, so the installed-only pass
    // below can skip them in O(1) instead of rescanning `games` each iteration.
    let mut owned_app_ids = std::collections::HashSet::new();

    // Games returned by the owned-games list are licensed to this account.
    for owned_game in owned {
        if is_ignored_steam_app(owned_game.app_id, &owned_game.name) {
            continue;
        }
        owned_app_ids.insert(owned_game.app_id);
        let info = installed_info.get(&owned_game.app_id);
        let install_path = info.map(|i| i.install_path.to_string_lossy().to_string());
        let active_branch = info
            .map(|i| i.active_branch.clone())
            .unwrap_or_else(|| "public".to_string());

        games.push(LibraryGame {
            app_id: owned_game.app_id,
            name: owned_game.name,
            playtime_forever_minutes: Some(owned_game.playtime_forever_minutes),
            is_installed: install_path.is_some(),
            install_path,
            local_manifest_ids: owned_game.local_manifest_ids,
            update_available: owned_game.update_available,
            update_queued: false,
            active_branch,
            is_owned: true,
            is_family_shared: false,
            online_required: None,
        });
    }

    // Anything installed but absent from the owned list is not licensed to this
    // account. If its appmanifest records a different owner, it's Family-Shared.
    for (app_id, info) in installed_info {
        if owned_app_ids.contains(&app_id) {
            continue;
        }
        // Skip Steam tooling (runtimes, Proton, redistributables) installed on disk.
        let candidate_name = info.name.as_deref().unwrap_or("");
        if is_ignored_steam_app(app_id, candidate_name) {
            continue;
        }

        // Only claim Family Sharing when we positively know the install belongs to
        // a different account. If we can't determine the owner (e.g. not logged in,
        // or the manifest has no LastOwner), don't guess — avoid false positives.
        let family_shared = matches!((info.last_owner, steam_id), (Some(owner), Some(me)) if owner != me);

        games.push(LibraryGame {
            app_id,
            name: info.name.unwrap_or_else(|| format!("App {app_id}")),
            playtime_forever_minutes: None,
            is_installed: true,
            install_path: Some(info.install_path.to_string_lossy().to_string()),
            local_manifest_ids: HashMap::new(),
            update_available: false,
            update_queued: false,
            active_branch: info.active_branch,
            is_owned: false,
            is_family_shared: family_shared,
            online_required: None,
        });
    }

    games.sort_by(|a, b| a.name.cmp(&b.name));
    GameLibrary { games }
}

pub fn merge_games(owned: Vec<OwnedGame>, installed: Vec<LocalGame>) -> Vec<GameModel> {
    let mut merged: HashMap<u32, GameModel> = HashMap::new();

    for game in owned {
        merged.insert(
            game.app_id,
            GameModel {
                app_id: game.app_id,
                name: game.name,
                playtime_forever_minutes: Some(game.playtime_forever_minutes),
                install_dir: None,
                proton_version: None,
                image_cache_path: None,
            },
        );
    }

    for local in installed {
        merged
            .entry(local.app_id)
            .and_modify(|existing| {
                existing.install_dir = Some(local.install_dir.clone());
                existing.proton_version = local.proton_version.clone();
                if existing.name.trim().is_empty() {
                    existing.name = local.name.clone();
                }
            })
            .or_insert(GameModel {
                app_id: local.app_id,
                name: local.name,
                playtime_forever_minutes: None,
                install_dir: Some(local.install_dir),
                proton_version: local.proton_version,
                image_cache_path: None,
            });
    }

    let mut games: Vec<GameModel> = merged.into_values().collect();
    games.sort_by(|a, b| a.name.cmp(&b.name));
    games
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_tooling_by_app_id() {
        assert!(is_ignored_steam_app(228980, "Steamworks Common Redistributables"));
        assert!(is_ignored_steam_app(1628350, "")); // Steam Linux Runtime 3.0
        assert!(is_ignored_steam_app(1493710, "Proton Experimental"));
    }

    #[test]
    fn ignores_tooling_by_name_prefix() {
        // App id not in the list, but the name marks it as tooling.
        assert!(is_ignored_steam_app(9999999, "Proton 9.0 (Beta)"));
        assert!(is_ignored_steam_app(9999998, "  Steam Linux Runtime 4.0"));
    }

    #[test]
    fn keeps_real_games() {
        assert!(!is_ignored_steam_app(620, "Portal 2"));
        // A game that merely contains "Proton" mid-name is not tooling.
        assert!(!is_ignored_steam_app(12345, "The Protonist"));
    }

    #[test]
    fn build_game_library_filters_tooling() {
        let owned = vec![
            OwnedGame {
                app_id: 620,
                name: "Portal 2".to_string(),
                playtime_forever_minutes: 0,
                local_manifest_ids: HashMap::new(),
                update_available: false,
            },
            OwnedGame {
                app_id: 228980,
                name: "Steamworks Common Redistributables".to_string(),
                playtime_forever_minutes: 0,
                local_manifest_ids: HashMap::new(),
                update_available: false,
            },
        ];
        let lib = build_game_library(owned, HashMap::new(), None);
        assert_eq!(lib.games.len(), 1);
        assert_eq!(lib.games[0].app_id, 620);
    }
}
