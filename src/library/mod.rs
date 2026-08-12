pub mod local_library;
pub mod collections;
pub mod depot_browser;
pub mod relocate;
pub mod cloud_sync;

use crate::core::config::{detect_steam_path, load_launcher_config};
use crate::core::models::{GameLibrary, GameModel, LibraryGame, LocalGame, OwnedGame};
use crate::core::utils::extract_quoted_values;
use anyhow::{Context, Result};
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

#[derive(Debug, Clone)]
pub struct InstalledAppInfo {
    pub install_path: PathBuf,
    pub active_branch: String,
    pub name: Option<String>,
    /// SteamID64 of the account that owns this local install (`LastOwner` in the
    /// appmanifest). Differs from the logged-in user for Family-Shared games.
    pub last_owner: Option<u64>,
    /// This install was found in the in-Wine Steam runtime's own library (inside the
    /// master prefix), not Aurelia's Linux library. Set by [`scan_installed_app_info`]
    /// when merging the Windows-Steam discovery pass.
    pub from_windows_steam: bool,
    /// The appmanifest this info came from. Kept so a duplicate install can be
    /// reported by the file that declares it.
    pub manifest_path: PathBuf,
    /// `LastUpdated` from the appmanifest — when Steam last wrote content for this
    /// copy. The primary signal for which of several copies is the live one.
    pub last_updated: u64,
    /// `buildid` from the appmanifest. Secondary tie-break behind `last_updated`.
    pub build_id: u64,
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

    // Scan *every* standard Steam data root, not just the first that exists: a
    // machine can carry a leftover native `~/.steam` next to the Flatpak install
    // that actually holds the games, and only one of them lists the libraries.
    let libraries = expand_library_roots(&steam_data_roots().await).await;
    tracing::debug!("scanning {} librar(y/ies): {:?}", libraries.len(), libraries);
    let mut installed = scan_libraries(&libraries).await;

    if config.is_some_and(|cfg| cfg.windows_steam_discovery_enabled) {
        let master_steam = crate::core::utils::get_master_steam_config();
        if master_steam.wine_prefix.exists() {
            tracing::debug!("scanning Windows Steam root: {:?}", master_steam.wine_prefix);
            // Windows Steam layout is drive_c/Program Files (x86)/Steam
            let windows_steam_root = master_steam.wine_prefix.join("drive_c/Program Files (x86)/Steam");
            if windows_steam_root.exists() {
                let windows_installed = scan_library_info(&windows_steam_root).await.unwrap_or_default();
                for (app_id, mut info) in windows_installed {
                    // Mark the source so `list` can flag it and `play` routes it through
                    // the in-Wine Steam. Prefer native/standard Linux Steam if duplicate.
                    info.from_windows_steam = true;
                    installed.entry(app_id).or_insert(info);
                }
            }
        }
    }

    Ok(installed)
}

/// One app installed in more than one library: the copy Aurelia uses, and the
/// redundant ones that can be deleted.
#[derive(Debug, Clone)]
pub struct DuplicateInstall {
    pub app_id: u32,
    pub name: Option<String>,
    pub live: InstalledAppInfo,
    /// Every other copy, newest first. Deleting these is what reconciles the app.
    pub stale: Vec<InstalledAppInfo>,
}

/// Find apps installed in several libraries at once.
///
/// A duplicate is not merely wasted disk: until it is removed the app has two
/// manifests disagreeing about what is installed, which is what makes an update
/// re-apply on every check.
pub async fn find_duplicate_installs() -> Result<Vec<DuplicateInstall>> {
    let libraries = expand_library_roots(&steam_data_roots().await).await;
    let mut duplicates: Vec<DuplicateInstall> = scan_libraries_all(&libraries)
        .await
        .into_iter()
        .filter(|(_, copies)| copies.len() > 1)
        .filter_map(|(app_id, mut copies)| {
            // Newest first, so the live copy is the head and the rest are stale.
            copies.sort_by_key(|c| std::cmp::Reverse((c.last_updated, c.build_id)));
            let live = copies.first()?.clone();
            let name = live.name.clone();
            Some(DuplicateInstall {
                app_id,
                name,
                live,
                stale: copies.into_iter().skip(1).collect(),
            })
        })
        .collect();
    duplicates.sort_by(|a, b| {
        a.name
            .as_deref()
            .unwrap_or("")
            .cmp(b.name.as_deref().unwrap_or(""))
            .then(a.app_id.cmp(&b.app_id))
    });
    Ok(duplicates)
}

/// The Steam data roots to scan: the configured library plus every standard
/// location. Shared by app discovery and duplicate reporting so both see the
/// same set of libraries.
async fn steam_data_roots() -> Vec<PathBuf> {
    let config = load_launcher_config().await.ok();
    let config_path = config.as_ref().and_then(|cfg| {
        let p = PathBuf::from(&cfg.steam_library_path);
        (p.join("steamapps").exists() || p.join("Steam").join("steamapps").exists()).then_some(p)
    });

    let mut roots: Vec<PathBuf> = config_path.into_iter().collect();
    roots.extend(steam_root_candidates());
    roots.sort();
    roots.dedup();
    roots
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
    let libraries = expand_library_roots(std::slice::from_ref(&root_path.to_path_buf())).await;
    Ok(scan_libraries(&libraries).await)
}

/// Expand Steam data roots into the full set of library folders to scan: each
/// root itself, every library its `libraryfolders.vdf` files register, and a
/// probe of the connected drives for libraries Steam never registered.
async fn expand_library_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut libraries: Vec<PathBuf> = roots.to_vec();

    for root in roots {
        for vdf in libraryfolders_vdf_paths(root) {
            match parse_library_folders(vdf.clone()).await {
                Ok(found) => libraries.extend(found),
                Err(e) => tracing::warn!("could not parse {}: {e}", vdf.display()),
            }
        }
    }

    // Steam doesn't always register every library in libraryfolders.vdf (and the
    // file may live on a root we never read). Probe the connected drives so games
    // on a secondary disk are found regardless.
    libraries.extend(discover_drive_libraries());

    // Resolve symlinks before deduping. A standard Steam install has `~/.steam/root`
    // and `~/.steam/steam` both pointing at `~/.local/share/Steam`, so a purely
    // textual dedupe leaves the same library listed three times: it gets scanned
    // three times, and every game in it looks installed in three libraries.
    libraries = libraries
        .into_iter()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect();

    libraries.sort();
    libraries.dedup();
    libraries
}

/// The places a Steam data root keeps its `libraryfolders.vdf`. Steam maintains
/// a copy under both `steamapps/` and `config/`, and either can be the fresher
/// one, so both are read.
fn libraryfolders_vdf_paths(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join("steamapps").join("libraryfolders.vdf"),
        root.join("config").join("libraryfolders.vdf"),
    ]
}

/// Read the appmanifests of a known set of library folders, keeping only the
/// live copy of each app.
///
/// The same game can be installed in several libraries at once — a leftover on an
/// external drive beside a fresh install on the internal one. Taking whichever
/// manifest happened to be read last makes the winner depend on the order the
/// libraries sorted in, which silently pins the app to a stale copy: updates then
/// download into the *configured* library while the check keeps reading the old
/// manifest, so the game reports an available update forever.
async fn scan_libraries(libraries: &[PathBuf]) -> HashMap<u32, InstalledAppInfo> {
    scan_libraries_all(libraries)
        .await
        .into_iter()
        .filter_map(|(app_id, copies)| pick_live_copy(copies).map(|info| (app_id, info)))
        .collect()
}

/// Of several installs of one app, the one to treat as live: most recently
/// updated, then highest build id. Both come straight from the appmanifest, so a
/// copy Steam or Aurelia has just written wins over one untouched for months.
/// Equal on both, the first (libraries are scanned in sorted order) is kept, so
/// the result stays deterministic.
fn pick_live_copy(copies: Vec<InstalledAppInfo>) -> Option<InstalledAppInfo> {
    copies
        .into_iter()
        .reduce(|best, next| {
            let better = (next.last_updated, next.build_id) > (best.last_updated, best.build_id);
            if better { next } else { best }
        })
}

/// Every install of every app across `libraries`, including duplicates. Backs both
/// [`scan_libraries`] and duplicate reporting.
pub async fn scan_libraries_all(libraries: &[PathBuf]) -> HashMap<u32, Vec<InstalledAppInfo>> {
    let mut installed: HashMap<u32, Vec<InstalledAppInfo>> = HashMap::new();

    for library_root in libraries {
        let steamapps = library_root.join("steamapps");
        if !steamapps.exists() {
            continue;
        }

        // A library we can't read (permissions, a drive unplugged mid-scan) must
        // not abort discovery of the others — that would hide every game behind
        // one bad mount.
        let mut dir = match fs::read_dir(&steamapps).await {
            Ok(dir) => dir,
            Err(e) => {
                tracing::warn!("skipping library {}: {e}", steamapps.display());
                continue;
            }
        };

        while let Ok(Some(entry)) = dir.next_entry().await {
            let path = entry.path();
            if !is_app_manifest(&path) {
                continue;
            }

            match parse_app_manifest_info(&path).await {
                Ok(Some((app_id, info))) => {
                    installed.entry(app_id).or_default().push(info);
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("skipping bad manifest {:?}: {}", path, e),
            }
        }
    }

    installed
}

/// How far below a volume root a library folder may sit before we stop looking.
/// Depth 2 covers a library at the volume root, one directory down, and one more
/// (`<volume>/Games/<library>`) — deeper nesting is rare enough that the cost of
/// walking for it outweighs the benefit.
const MAX_PROBE_DEPTH: usize = 2;

/// Cap on directory entries examined per level, so a volume with an enormous
/// top-level directory count can't turn discovery into a full filesystem walk.
const MAX_PROBE_ENTRIES: usize = 512;

/// Directory names never worth descending into: the OS trees on a root
/// filesystem, and the bookkeeping directories Windows leaves on removable
/// volumes. Names Steam itself uses are deliberately absent — a library is
/// recognised by its structure, not its name.
const PROBE_SKIP_DIRS: &[&str] = &[
    "bin", "boot", "dev", "etc", "lib", "lib32", "lib64", "libx32", "proc", "root", "run", "sbin",
    "srv", "sys", "tmp", "usr", "var", "lost+found", "$RECYCLE.BIN", "System Volume Information",
    "Recovery", "Windows", "WinSxS", "AppData", "node_modules",
];

/// Probe every connected drive for Steam library folders.
///
/// Libraries are frequently missing from `libraryfolders.vdf` (or listed only in
/// a `.vdf` on a root we never read), so each mounted volume is searched
/// directly. A library is identified **structurally** — a directory containing
/// `steamapps/` — rather than by name, because the folder name is the user's
/// free choice (`SteamLibrary`, `Games`, `Spiele`, …) and guessing names only
/// ever finds the layouts that happen to be guessed.
pub fn discover_drive_libraries() -> Vec<PathBuf> {
    let mut found = Vec::new();

    for volume in volume_roots() {
        probe_for_libraries(&volume, MAX_PROBE_DEPTH, &mut found);
    }

    found.sort();
    found.dedup();
    found
}

/// Recursively look for directories holding a `steamapps/` subdirectory, down to
/// `depth` levels below `dir`.
fn probe_for_libraries(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if dir.join("steamapps").is_dir() {
        found.push(dir.to_path_buf());
        // Steam never nests one library inside another, so stop descending here.
        return;
    }
    if depth == 0 {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        // Unreadable (permissions, unplugged mid-probe) — nothing to report.
        return;
    };

    for entry in entries.flatten().take(MAX_PROBE_ENTRIES) {
        // `is_dir()` on the entry's own file type, so symlinks are not followed:
        // a link pointing back up the tree would otherwise loop.
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Hidden directories hold caches and dotfile trees, not game libraries.
        // The Steam data roots that *are* hidden are covered by
        // [`steam_root_candidates`], not by this probe.
        if name.starts_with('.') || PROBE_SKIP_DIRS.iter().any(|skip| name.eq_ignore_ascii_case(skip))
        {
            continue;
        }
        probe_for_libraries(&entry.path(), depth - 1, found);
    }
}

/// Roots of the volumes that could hold a Steam library.
fn volume_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let mut roots = Vec::new();
        for letter in b'A'..=b'Z' {
            let drive = PathBuf::from(format!("{}:\\", letter as char));
            if drive.exists() {
                roots.push(drive);
            }
        }
        roots
    }

    #[cfg(not(target_os = "windows"))]
    {
        mounted_volume_roots()
    }
}

/// Roots of mounted, real (non-pseudo) filesystems that could hold a Steam
/// library: every disk-backed mount point from `/proc/self/mounts`, plus the
/// conventional removable-media parents for hosts without `/proc` (macOS).
#[cfg(not(target_os = "windows"))]
fn mounted_volume_roots() -> Vec<PathBuf> {
    // Filesystems that can hold a game install. Everything else a Linux box
    // mounts (proc, sysfs, cgroup, tmpfs, overlay, …) cannot, and walking them
    // just costs syscalls.
    const DISK_FSTYPES: &[&str] = &[
        "ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "jfs", "reiserfs", "zfs", "bcachefs",
        "ntfs", "ntfs3", "fuseblk", "exfat", "vfat", "msdos", "udf", "apfs", "hfs", "hfsplus",
        "nfs", "nfs4", "cifs", "smb3",
    ];

    let mut roots: Vec<PathBuf> = Vec::new();

    if let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") {
        for line in mounts.lines() {
            let mut fields = line.split_whitespace();
            let (Some(_source), Some(target), Some(fstype)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if !DISK_FSTYPES.contains(&fstype) {
                continue;
            }
            roots.push(PathBuf::from(unescape_mount_target(target)));
        }
    }

    // Removable volumes are conventionally mounted one or two levels under
    // these (`/media/<label>`, `/run/media/<user>/<label>`, `/Volumes/<label>`).
    // Redundant with /proc on Linux; the only source on macOS.
    for parent in ["/Volumes", "/run/media", "/media", "/mnt"] {
        let Ok(entries) = std::fs::read_dir(parent) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Ok(nested) = std::fs::read_dir(&path) {
                roots.extend(nested.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
            }
            roots.push(path);
        }
    }

    roots.sort();
    roots.dedup();
    roots
}

/// Decode the octal escapes (`\040` for space, `\134` for backslash, …) that
/// `/proc/self/mounts` uses in mount-point paths.
#[cfg(not(target_os = "windows"))]
fn unescape_mount_target(target: &str) -> String {
    if !target.contains('\\') {
        return target.to_string();
    }
    let bytes = target.as_bytes();
    // Decoded byte-wise, not char-wise: an escape yields one raw byte, which may
    // be part of a multi-byte UTF-8 sequence in the mount point's name.
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = (bytes[i] == b'\\' && i + 3 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 4]).ok())
            .flatten()
            .and_then(|digits| u8::from_str_radix(digits, 8).ok());
        match octal {
            Some(byte) => {
                out.push(byte);
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Collect every Steam library root we can discover: the configured library,
/// every standard Steam data root, anything their `libraryfolders.vdf` files
/// reference, and a probe of all connected drives.
pub async fn all_library_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    if let Ok(cfg) = load_launcher_config().await {
        let p = PathBuf::from(&cfg.steam_library_path);
        if !p.as_os_str().is_empty() {
            roots.push(p);
        }
    }
    roots.extend(steam_root_candidates());

    expand_library_roots(&roots).await
}

/// Every standard location a Steam data root can live in on this platform.
///
/// All of them are returned, not just the first that exists: the packaging
/// formats install side by side, and a machine can easily carry a stale native
/// `~/.steam` next to the Flatpak or Snap install that holds the real library.
pub fn steam_root_candidates() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    #[cfg(target_os = "windows")]
    {
        for var in ["PROGRAMFILES(X86)", "PROGRAMFILES"] {
            if let Ok(program_files) = std::env::var(var) {
                roots.push(PathBuf::from(program_files).join("Steam"));
            }
        }
        roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(home) = crate::core::config::home_dir() {
            // Native installs. `.steam/steam` and `.steam/root` are symlinks
            // Steam maintains into the real data directory.
            for relative in [
                ".steam/steam",
                ".steam/root",
                ".local/share/Steam",
                // Debian/Ubuntu's steam package uses its own data directory.
                ".steam/debian-installation",
                // Flatpak (com.valvesoftware.Steam) and Snap sandbox their $HOME.
                ".var/app/com.valvesoftware.Steam/.local/share/Steam",
                ".var/app/com.valvesoftware.Steam/.steam/steam",
                "snap/steam/common/.local/share/Steam",
                "snap/steam/common/.steam/steam",
                // macOS.
                "Library/Application Support/Steam",
            ] {
                roots.push(home.join(relative));
            }
        }
        // Honour an XDG data directory that isn't the default `~/.local/share`.
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            roots.push(PathBuf::from(xdg).join("Steam"));
        }
    }

    // Whatever the platform-specific detection finds, in case it knows a location
    // this list does not.
    roots.extend(detect_steam_path());

    roots.retain(|root| root.exists());
    roots.sort();
    roots.dedup();
    roots
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

    let libraries = library_folder_paths(&raw);
    tracing::debug!(
        "libraryfolders.vdf {} lists {} librar{}",
        path.display(),
        libraries.len(),
        if libraries.len() == 1 { "y" } else { "ies" }
    );
    Ok(libraries)
}

/// Extract the library roots listed in a `libraryfolders.vdf` body.
///
/// Hand-rolled rather than deserialized: the file has two historical shapes —
/// modern (`"0" { "path" "…" "apps" { … } }`) and legacy pre-2021 (`"1"
/// "D:\\SteamLibrary"`) — and a `#[serde(untagged)]` enum spanning both matched
/// *neither* on real files, silently yielding zero libraries. That left every
/// scan seeing only the root it started from, so games on secondary drives were
/// never found.
pub fn library_folder_paths(raw: &str) -> Vec<PathBuf> {
    fn is_index(key: &str) -> bool {
        !key.is_empty() && key.chars().all(|ch| ch.is_ascii_digit())
    }

    let mut libraries: Vec<PathBuf> = Vec::new();
    // Key that opened each block we are currently nested inside. Depth 1 is the
    // `"libraryfolders"` block's body, depth 2 a numbered library's keys.
    let mut block_keys: Vec<String> = Vec::new();
    let mut pending_key: Option<String> = None;

    let mut push = |value: &str| {
        let unescaped = unescape_vdf(value);
        if !unescaped.is_empty() {
            libraries.push(PathBuf::from(unescaped));
        }
    };

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if trimmed == "{" {
            block_keys.push(pending_key.take().unwrap_or_default());
            continue;
        }
        if trimmed == "}" {
            block_keys.pop();
            pending_key = None;
            continue;
        }

        let parts = extract_quoted_values(trimmed);
        match parts.as_slice() {
            // A lone key names the block that the next `{` opens.
            [key] => pending_key = Some(key.clone()),
            [key, value, ..] => {
                let depth = block_keys.len();
                if depth == 1 && is_index(key) {
                    // Legacy flat form: the index maps straight to a path.
                    push(value);
                } else if depth == 2
                    && key.eq_ignore_ascii_case("path")
                    && block_keys.last().is_some_and(|k| is_index(k))
                {
                    // Modern form. Requiring an indexed parent keeps us out of
                    // the sibling `apps` block (appid -> byte size).
                    push(value);
                }
            }
            [] => {}
        }
    }

    libraries.sort();
    libraries.dedup();
    libraries
}

/// Undo VDF string escaping (`\\`, `\"`, `\t`, `\n`). Steam writes Windows
/// library paths as `D:\\SteamLibrary`; left as-is the doubled separators break
/// path comparisons against the same library discovered another way.
fn unescape_vdf(value: &str) -> String {
    if !value.contains('\\') {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            // Not a recognised escape — keep both characters verbatim.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
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
    let mut state_flags: Option<u32> = None;
    let mut last_updated: u64 = 0;
    let mut build_id: u64 = 0;

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
                    "stateflags" => state_flags = value.parse::<u32>().ok(),
                    "lastupdated" => last_updated = value.parse::<u64>().unwrap_or(0),
                    "buildid" => build_id = value.parse::<u64>().unwrap_or(0),
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

    // Only count an app as installed once its manifest is marked fully installed
    // (StateFlags & StateFullyInstalled). A manifest written at install start
    // carries only StateUpdateRequired (2); if the install is cancelled that
    // partial manifest remains, and without this check the game would wrongly be
    // reported as installed by `list`.
    if !state_flags.is_some_and(|flags| flags & 4 != 0) {
        return Ok(None);
    }
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
            from_windows_steam: false,
            manifest_path: path.to_path_buf(),
            last_updated,
            build_id,
        },
    )))
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
        let from_windows_steam = info.is_some_and(|i| i.from_windows_steam);
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
            platform: None,
            from_windows_steam,
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
        let from_windows_steam = info.from_windows_steam;

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
            platform: None,
            from_windows_steam,
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
#[path = "library_tests.rs"]
mod tests;
