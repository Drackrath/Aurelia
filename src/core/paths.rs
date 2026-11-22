//! Prefix, library and path resolution.

use std::path::{Path, PathBuf};

use crate::core::steam_host::resolve_master_wineprefix;

/// True only for one plain directory name.
pub fn is_safe_dir_name(name: &str) -> bool {
    let mut parts = Path::new(name).components();
    !name.trim().is_empty()
        && !name.contains(['/', '\\'])
        && matches!(
            (parts.next(), parts.next()),
            (Some(std::path::Component::Normal(_)), None)
        )
}

/// Relative path with no escaping parts.
pub fn safe_relative_path(path: &str) -> Option<PathBuf> {
    use std::path::Component;
    if path.contains('\0') {
        return None;
    }
    let mut out = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Nearest existing ancestor resolves inside `root`.
pub fn resolves_within(root: &Path, path: &Path) -> bool {
    let mut probe = path;
    loop {
        if std::fs::symlink_metadata(probe).is_ok() {
            return probe.canonicalize().is_ok_and(|p| p.starts_with(root));
        }
        match probe.parent() {
            Some(parent) => probe = parent,
            None => return false,
        }
    }
}

/// [`wineprefix_for_game`] resolved through a full store.
pub fn steam_wineprefix_for_game(
    config: &crate::core::config::LauncherConfig,
    app_id: u32,
    user_configs: &crate::core::models::UserConfigStore,
    install_path: Option<&Path>,
) -> std::path::PathBuf {
    wineprefix_for_game(config, app_id, user_configs.get(&app_id), install_path)
}

pub fn wineprefix_for_game(
    config: &crate::core::config::LauncherConfig,
    app_id: u32,
    user_config: Option<&crate::core::models::UserAppConfig>,
    install_path: Option<&Path>,
) -> std::path::PathBuf {
    // Custom prefix is always per-game.
    if let Some(dir) = config.prefix_dir(app_id) {
        return dir.join("pfx");
    }
    let use_steam_runtime = matches!(
        user_config.map(|c| &c.steam_runtime_policy),
        Some(crate::core::models::SteamRuntimePolicy::Enabled)
    );

    let use_per_game_compat_data = user_config
        .map(|c| {
            use_steam_runtime
                && c.steam_prefix_mode == crate::core::models::SteamPrefixMode::PerGame
        })
        .unwrap_or(config.use_shared_compat_data);

    if use_per_game_compat_data {
        proton_compat_prefix(config, app_id, install_path)
    } else {
        resolve_master_wineprefix()
    }
}

/// The prefix Proton itself uses for a game: `$STEAM_COMPAT_DATA_PATH/pfx`, where
/// `STEAM_COMPAT_DATA_PATH` is always `<library>/steamapps/compatdata/<appid>`.
pub fn proton_compat_prefix(
    config: &crate::core::config::LauncherConfig,
    app_id: u32,
    install_path: Option<&Path>,
) -> std::path::PathBuf {
    game_compat_data_dir(config, app_id, install_path).join("pfx")
}

/// Compatdata dir in the game's library.
pub fn game_compat_data_dir(
    config: &crate::core::config::LauncherConfig,
    app_id: u32,
    install_path: Option<&Path>,
) -> PathBuf {
    if let Some(dir) = config.prefix_dir(app_id) {
        return dir.to_path_buf();
    }
    let configured = Path::new(&config.steam_library_path);
    let root = game_library_root(config, app_id, install_path);
    let dir = compat_data_dir(&root, app_id);
    if root != configured {
        let old = compat_data_dir(configured, app_id);
        if old.is_dir() && !dir.exists() {
            tracing::warn!(
                "app {app_id}: prefix now lives in its own library ({}); \
                 an older prefix exists at {} (move it there to keep saves)",
                dir.display(),
                old.display()
            );
        }
    }
    dir
}

/// Library holding the game's appmanifest.
pub fn game_library_root(
    config: &crate::core::config::LauncherConfig,
    app_id: u32,
    install_path: Option<&Path>,
) -> PathBuf {
    // Install path beats the library walk.
    if let Some(lib) = install_path.and_then(|p| library_from_install_path(p, app_id)) {
        return lib;
    }
    game_library_root_in(
        Path::new(&config.steam_library_path),
        &crate::library::steam_root_candidates(),
        app_id,
    )
}

/// Install dir's library, if manifest present.
pub fn library_from_install_path(install_path: &Path, app_id: u32) -> Option<PathBuf> {
    let common = install_path.parent()?;
    let steamapps = common.parent()?;
    let is = |p: &Path, name: &str| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
    };
    if !is(common, "common") || !is(steamapps, "steamapps") {
        return None;
    }
    steamapps
        .join(format!("appmanifest_{app_id}.acf"))
        .is_file()
        .then(|| steamapps.parent().map(Path::to_path_buf))
        .flatten()
}

/// Testable core of [`game_library_root`].
pub fn game_library_root_in(configured: &Path, steam_roots: &[PathBuf], app_id: u32) -> PathBuf {
    let manifest = format!("appmanifest_{app_id}.acf");
    let has_manifest = |lib: &Path| lib.join("steamapps").join(&manifest).is_file();
    if has_manifest(configured) {
        return configured.to_path_buf();
    }
    let mut roots = vec![configured.to_path_buf()];
    roots.extend(steam_roots.iter().cloned());
    let mut candidates: Vec<PathBuf> = steam_roots.to_vec();
    for root in &roots {
        for vdf in [
            root.join("steamapps").join("libraryfolders.vdf"),
            root.join("config").join("libraryfolders.vdf"),
        ] {
            if let Ok(raw) = std::fs::read_to_string(&vdf) {
                candidates.extend(crate::library::library_folder_paths(&raw));
            }
        }
    }
    candidates
        .into_iter()
        .find(|lib| has_manifest(lib))
        .unwrap_or_else(|| configured.to_path_buf())
}

/// `steamapps/compatdata/<appid>` under a library root.
pub fn compat_data_dir(library_root: &Path, app_id: u32) -> PathBuf {
    library_root
        .join("steamapps")
        .join("compatdata")
        .join(app_id.to_string())
}

/// The wine prefix a game's Windows files — saves included — actually live in.
///
/// This is *not* always [`steam_wineprefix_for_game`]. That returns the `WINEPREFIX`
/// the launcher sets, but a Proton launch ignores it: `proton run` derives its prefix
/// from `STEAM_COMPAT_DATA_PATH`, which is always `<library>/steamapps/compatdata/
/// <appid>` regardless of prefix mode (the same asymmetry documented on
/// [`resolve_steam_runtime_wine`](crate::core::runner::resolve_steam_runtime_wine)). In the default `Shared` mode the two therefore
/// disagree: the launcher points `WINEPREFIX` at the master prefix while the game
/// runs — and writes its saves — in compatdata.
///
/// Anything that needs to *find a game's files on disk* (Steam Cloud sync above all)
/// must ask this, not the `WINEPREFIX`. Getting it wrong is silent: saves are written
/// to a prefix the game never reads, and it starts with no progress.
///
/// Proton having set up `pfx/drive_c` there is the evidence that it is the live
/// prefix — the bare compatdata directory alone isn't, since `prepare_prefix`
/// pre-creates that for every launch, Proton or not. Without it, fall back to the
/// configured `WINEPREFIX`, which is what a bare-wine launch honours.
pub fn game_save_prefix(
    config: &crate::core::config::LauncherConfig,
    app_id: u32,
    user_configs: &crate::core::models::UserConfigStore,
    install_path: Option<&Path>,
) -> std::path::PathBuf {
    let proton_prefix = proton_compat_prefix(config, app_id, install_path);
    if proton_prefix.join("drive_c").is_dir() {
        return proton_prefix;
    }
    steam_wineprefix_for_game(config, app_id, user_configs, install_path)
}

/// Resolve `rel` under `base`, ignoring case.
///
/// Exact component matches win; `None` if missing. Names come from the directory
/// listing, not `exists()`: on a case-insensitive filesystem (NTFS, exFAT) any
/// spelling "exists", which would hand back the caller's spelling instead.
pub fn resolve_path_ignore_case(base: &Path, rel: &str) -> Option<PathBuf> {
    let mut current = if rel.starts_with('/') {
        PathBuf::from("/")
    } else {
        base.to_path_buf()
    };
    for part in rel
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
    {
        let mut matches: Vec<PathBuf> = std::fs::read_dir(&current)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(part))
            })
            .map(|e| e.path())
            .collect();
        let exact = current.join(part);
        current = if let Some(i) = matches
            .iter()
            .position(|m| m.file_name() == exact.file_name())
        {
            matches.swap_remove(i)
        } else if !matches.is_empty() {
            // Stable pick among case variants.
            matches.sort();
            matches.swap_remove(0)
        } else if exact.exists() {
            // Never listed, e.g. `..`.
            exact
        } else {
            return None;
        };
    }
    current.exists().then_some(current)
}

#[cfg(test)]
mod tests;
