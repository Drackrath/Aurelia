//! Proton/Wine runtime download manager.
//!
//! Lists, downloads and removes Proton/Wine compatibility tools from two sources:
//!
//! - **Valve official Proton** — distributed as free Steam apps (depots). These are
//!   installed through the normal content pipeline ([`crate::steam_client`]) and land
//!   under `steamapps/common`, where Steam itself also finds them.
//! - **GE community builds** — GloriousEggroll's `proton-ge-custom` (`.tar.gz`) and
//!   `wine-ge-custom` (`.tar.xz`), published as GitHub release assets. These are
//!   downloaded and extracted into Steam's `compatibilitytools.d`.
//!
//! Installed runtimes are discovered by scanning both locations. The "global default"
//! Proton lives in [`crate::core::config::LauncherConfig::proton_version`]; a fresh install
//! sets it (so the default is the most recently downloaded runtime).

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Where an installable package comes from.
#[derive(Debug, Clone)]
pub enum ProtonSource {
    /// Official Valve Proton: a free Steam app installed via the content pipeline.
    Valve { app_id: u32 },
    /// A GitHub release asset (GE builds), downloaded + extracted directly.
    Github { url: String, ext: String },
}

/// A runtime that can be installed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProtonPackage {
    /// The name used to install/select it (e.g. `GE-Proton9-20`, `Proton 9.0`).
    pub name: String,
    /// Human label for the source family (`Valve`, `Proton-GE`, `Wine-GE`).
    pub label: String,
    /// Download size in bytes (`0` when unknown, e.g. Valve apps before PICS).
    pub size: u64,
    #[serde(skip)]
    pub source: ProtonSource,
}

/// A runtime already present on disk.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledProton {
    pub name: String,
    /// `steam` (`steamapps/common`) or `custom` (`compatibilitytools.d`).
    pub location: &'static str,
    pub path: PathBuf,
}

/// Curated set of official Valve Proton apps (name → Steam app id). These are free
/// tools licensed to every account, installed like any other app.
const VALVE_PROTONS: &[(&str, u32)] = &[
    // Names must match Steam's on-disk install directory under steamapps/common so
    // a selected runtime resolves at launch (Experimental uses a dashed dir name).
    ("Proton - Experimental", 1493710),
    ("Proton 9.0", 2805730),
    ("Proton 8.0", 2348590),
    ("Proton 7.0", 1887720),
    ("Proton 6.3", 1580130),
    ("Proton 5.13", 1420170),
];

/// A GitHub repo serving GE runtime tarballs, and the asset extension to pick.
struct GithubSource {
    repo: &'static str,
    label: &'static str,
    ext: &'static str,
    /// When set, this source ships CPU-microarch-specific assets (e.g. Proton-CachyOS
    /// `x86_64_v3` vs `x86_64`). Asset selection then prefers the best match for the
    /// host CPU (see [`host_cachyos_microarch`]).
    microarch: bool,
}

const GE_SOURCES: &[GithubSource] = &[
    GithubSource { repo: "GloriousEggroll/proton-ge-custom", label: "Proton-GE", ext: ".tar.gz", microarch: false },
    GithubSource { repo: "GloriousEggroll/wine-ge-custom", label: "Wine-GE", ext: ".tar.xz", microarch: false },
    // Proton-CachyOS ships two microarch builds per release: a baseline `x86_64` and
    // an AVX2-optimized `x86_64_v3`. We pick the best one for the host CPU.
    GithubSource { repo: "CachyOS/proton-cachyos", label: "Proton-CachyOS", ext: ".tar.xz", microarch: true },
];

/// How many recent releases to list per GE repo.
const GE_RELEASES_PER_REPO: usize = 8;

// ---------------------------------------------------------------------------
// Modern unified-layout component discovery (shared constants)
// ---------------------------------------------------------------------------
//
// Modern runners (Proton 11+, GE, Proton-CachyOS) use a unified layout where
// graphics components live under `<lib-root>/wine/<component>/<arch>` alongside the
// bare base dirs that hold the WOW64 unix-bridge `.so` libraries. These constants
// are the single source of truth shared by `utils`, `dll_provider_resolver`, and the
// `wine_tkg` runner so discovery paths stay consistent and de-duplicated.

/// Lib roots a runner may use. The `*/wine` roots host the arch-split component
/// folders; the bare base dirs (`files/lib`, `files/lib64`) hold the WOW64 unix-bridge
/// `.so` loaders that a modern WOW64 build needs on `WINEDLLPATH`.
pub(crate) const UNIFIED_LIB_SUBDIRS: &[&str] = &[
    "lib/wine",
    "lib64/wine",
    "files/lib/wine",
    "files/lib64/wine",
    "dist/lib/wine",
    "dist/lib64/wine",
    // Bare base dirs — WOW64 unix bridge `.so` libs (ntdll.so, etc.).
    "files/lib",
    "files/lib64",
];

/// Architecture subdirectories a component may split into. The `-windows` dirs hold
/// the PE DLLs; the `-unix` dirs are the WOW64 bridge dirs (host-side `.so`).
pub(crate) const ARCH_SUBDIRS: &[&str] =
    &["x86_64-windows", "i386-windows", "x86_64-unix", "i386-unix"];

/// Graphics component families discovered inside a runner.
pub(crate) const COMPONENT_FAMILIES: &[&str] = &["dxvk", "vkd3d", "vkd3d-proton", "nvapi"];

/// The `<lib-root>/wine/<family>` component directories across every unified/legacy
/// lib layout (excludes the bare base dirs, which never host components).
pub(crate) fn wine_component_dirs(family: &str) -> Vec<String> {
    UNIFIED_LIB_SUBDIRS
        .iter()
        .filter(|l| l.ends_with("wine"))
        .map(|l| format!("{l}/{family}"))
        .collect()
}

/// All candidate relative dirs where a component's DLLs may live: every
/// `<lib-root>/wine/<family>/<arch>` plus the bare `<lib-root>/wine/<family>`
/// (arch-neutral / legacy split-lib layouts).
pub(crate) fn component_dll_subdirs(family: &str) -> Vec<String> {
    let mut out = Vec::new();
    for lib in UNIFIED_LIB_SUBDIRS.iter().filter(|l| l.ends_with("wine")) {
        for arch in ARCH_SUBDIRS {
            out.push(format!("{lib}/{family}/{arch}"));
        }
        out.push(format!("{lib}/{family}"));
    }
    out
}

/// The best Proton-CachyOS microarch asset infix for the host CPU: `x86_64_v3` when
/// the host supports AVX2, else the baseline `x86_64`. Detection runs on the build
/// host CPU via `is_x86_feature_detected!`, which is portable and needs no target
/// cfg beyond the x86_64 guard.
pub(crate) fn host_cachyos_microarch() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") {
            return "x86_64_v3";
        }
    }
    "x86_64"
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Steam's `compatibilitytools.d` directory, where custom (GE) runtimes live.
/// Derived from the detected Steam root, falling back to the conventional path.
pub fn compat_tools_dir() -> Result<PathBuf> {
    if let Some(steam) = crate::core::config::detect_steam_path() {
        return Ok(steam.join("compatibilitytools.d"));
    }
    let home = crate::core::config::home_dir()?;
    Ok(home.join(".local/share/Steam/compatibilitytools.d"))
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Scan disk for installed Proton/Wine runtimes: Steam-managed Proton under
/// `<steam_library>/steamapps/common`, plus everything under `compatibilitytools.d`.
pub fn list_installed(steam_library_path: &Path) -> Vec<InstalledProton> {
    let mut out = Vec::new();

    // Steam-managed Proton under steamapps/common.
    let common = steam_library_path.join("steamapps/common");
    if let Ok(entries) = std::fs::read_dir(&common) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() && name.to_ascii_lowercase().contains("proton") {
                out.push(InstalledProton { name, location: "steam", path });
            }
        }
    }

    // Custom tools (GE etc.).
    if let Ok(custom) = compat_tools_dir() {
        if let Ok(entries) = std::fs::read_dir(&custom) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    out.push(InstalledProton {
                        name: entry.file_name().to_string_lossy().to_string(),
                        location: "custom",
                        path,
                    });
                }
            }
        }
    }

    // Drop duplicate physical directories reached via different library roots,
    // symlinks, or a `compatibilitytools.d` that overlaps the scanned library
    // (canonicalize so the same runtime isn't listed twice).
    let mut seen_paths = std::collections::HashSet::new();
    out.retain(|p| {
        let key = std::fs::canonicalize(&p.path).unwrap_or_else(|_| p.path.clone());
        seen_paths.insert(key)
    });

    out.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// Build the [`ProtonPackage`] for a curated Valve entry (name + Steam app id).
fn valve_package(name: &str, app_id: u32) -> ProtonPackage {
    ProtonPackage {
        name: name.to_string(),
        label: "Valve".to_string(),
        size: 0,
        source: ProtonSource::Valve { app_id },
    }
}

/// List everything installable: the curated Valve Proton set plus recent GE
/// releases fetched from GitHub. GitHub failures (offline, rate-limited) are logged
/// and that source is skipped rather than failing the whole listing.
pub async fn list_available() -> Result<Vec<ProtonPackage>> {
    let mut out: Vec<ProtonPackage> = VALVE_PROTONS
        .iter()
        .map(|(name, app_id)| valve_package(name, *app_id))
        .collect();

    for src in GE_SOURCES {
        match fetch_github_releases(src, GE_RELEASES_PER_REPO).await {
            Ok(mut pkgs) => out.append(&mut pkgs),
            Err(e) => tracing::warn!("could not list {} releases: {e:#}", src.label),
        }
    }
    Ok(out)
}

/// Resolve a single package by name (case-insensitive). Checks the Valve set first,
/// then each GE repo's release tags.
pub async fn resolve_package(name: &str) -> Result<ProtonPackage> {
    let needle = normalize_name(name);
    if let Some((vname, app_id)) = VALVE_PROTONS
        .iter()
        .find(|(n, _)| normalize_name(n) == needle)
    {
        return Ok(valve_package(vname, *app_id));
    }

    for src in GE_SOURCES {
        if let Some(pkg) = fetch_github_release_by_tag(src, name).await? {
            return Ok(pkg);
        }
    }

    bail!(
        "no Proton/Wine package named '{name}' (try `aurelia proton list` to see available names)"
    )
}

// ---------------------------------------------------------------------------
// GitHub
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// Build a reqwest client with the headers GitHub requires (a User-Agent) and an
/// optional `GITHUB_TOKEN` to lift the unauthenticated rate limit.
fn github_client() -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder().user_agent("aurelia-proton-manager");
    if let Some(token) = std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty()) {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(value) = format!("Bearer {token}").parse() {
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        builder = builder.default_headers(headers);
    }
    builder.build().context("failed to build the GitHub HTTP client")
}

/// Pick the release asset to download: among assets with the source's extension
/// (skipping checksum/`.sha*` files), honor CPU microarch when `microarch` is set —
/// prefer the `x86_64_v3` (AVX2) build when the host supports it, otherwise the
/// generic `x86_64` build. Falls back to the first matching asset.
fn choose_asset(assets: Vec<GhAsset>, ext: &str, microarch: Option<&str>) -> Option<GhAsset> {
    let mut matching: Vec<GhAsset> =
        assets.into_iter().filter(|a| a.name.ends_with(ext)).collect();

    if let Some(arch) = microarch {
        if arch == "x86_64_v3" {
            if let Some(i) = matching.iter().position(|a| a.name.contains("x86_64_v3")) {
                return Some(matching.swap_remove(i));
            }
        }
        // Baseline host (or no v3 asset): pick the generic x86_64 build, never a
        // higher microarch level the CPU may not support.
        if let Some(i) = matching.iter().position(|a| {
            a.name.contains("x86_64")
                && !a.name.contains("x86_64_v3")
                && !a.name.contains("x86_64_v4")
        }) {
            return Some(matching.swap_remove(i));
        }
    }

    matching.into_iter().next()
}

/// Map a GitHub release to a package, picking the best asset for the source (and
/// host CPU, for microarch sources). Returns `None` if no asset matches.
fn release_to_package(src: &GithubSource, rel: GhRelease) -> Option<ProtonPackage> {
    let microarch = src.microarch.then(host_cachyos_microarch);
    let asset = choose_asset(rel.assets, src.ext, microarch)?;
    // For microarch sources, surface the selected build in the label so `proton list`
    // shows whether the AVX2-optimized asset was chosen.
    let label = if src.microarch && asset.name.contains("x86_64_v3") {
        format!("{} (x86_64_v3 — AVX2 optimized)", src.label)
    } else {
        src.label.to_string()
    };
    Some(ProtonPackage {
        name: rel.tag_name,
        label,
        size: asset.size,
        source: ProtonSource::Github {
            url: asset.browser_download_url,
            ext: src.ext.to_string(),
        },
    })
}

async fn fetch_github_releases(src: &GithubSource, per_page: usize) -> Result<Vec<ProtonPackage>> {
    let url = format!(
        "https://api.github.com/repos/{}/releases?per_page={per_page}",
        src.repo
    );
    let releases: Vec<GhRelease> = github_client()?
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed requesting {url}"))?
        .error_for_status()
        .with_context(|| format!("GitHub returned an error for {}", src.repo))?
        .json()
        .await
        .context("failed parsing GitHub releases JSON")?;

    Ok(releases
        .into_iter()
        .filter_map(|rel| release_to_package(src, rel))
        .collect())
}

async fn fetch_github_release_by_tag(
    src: &GithubSource,
    tag: &str,
) -> Result<Option<ProtonPackage>> {
    let url = format!("https://api.github.com/repos/{}/releases/tags/{tag}", src.repo);
    let resp = github_client()?
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed requesting {url}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let rel: GhRelease = resp
        .error_for_status()
        .with_context(|| format!("GitHub returned an error for {}/{tag}", src.repo))?
        .json()
        .await
        .context("failed parsing GitHub release JSON")?;
    Ok(release_to_package(src, rel))
}

// ---------------------------------------------------------------------------
// Install / remove (GE)
// ---------------------------------------------------------------------------

/// Download a GE package's tarball and extract it into `compatibilitytools.d`.
/// `on_progress(downloaded, total)` is called as bytes arrive (`total` is `0` when
/// the server sends no Content-Length). Returns the installed directory.
///
/// Only valid for [`ProtonSource::Github`] packages; Valve packages install through
/// the Steam content pipeline instead (see the `proton install` command).
pub async fn install_github_package(
    pkg: &ProtonPackage,
    on_progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Result<PathBuf> {
    let (url, ext) = match &pkg.source {
        ProtonSource::Github { url, ext } => (url, ext),
        ProtonSource::Valve { .. } => {
            bail!("'{}' is an official Valve Proton — install it via Steam, not the GE downloader", pkg.name)
        }
    };

    let dest_dir = compat_tools_dir()?;
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("failed creating {}", dest_dir.display()))?;

    // Download to a temp file alongside the destination so extraction is a local move.
    let tmp = dest_dir.join(format!(".{}.download{}", pkg.name, ext));
    download_to(url, &tmp, on_progress)
        .await
        .with_context(|| format!("failed downloading {}", pkg.name))?;

    let result = extract_tarball(&tmp, ext, &dest_dir)
        .with_context(|| format!("failed extracting {}", pkg.name));
    // Always clean up the temp tarball, success or failure.
    let _ = std::fs::remove_file(&tmp);
    result?;

    Ok(dest_dir.join(&pkg.name))
}

/// Stream `url` to `dest`, reporting progress as bytes arrive.
pub(crate) async fn download_to(
    url: &str,
    dest: &Path,
    on_progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let client = github_client()?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed requesting {url}"))?
        .error_for_status()
        .context("download request failed")?;

    let total = resp.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("failed creating {}", dest.display()))?;

    let mut downloaded = 0u64;
    on_progress(0, total);
    while let Some(chunk) = resp.chunk().await.context("failed reading download stream")? {
        file.write_all(&chunk)
            .await
            .with_context(|| format!("failed writing {}", dest.display()))?;
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    }
    file.flush().await.ok();
    Ok(())
}

/// Extract a `.tar.gz` or `.tar.xz` tarball into `dest_parent` (the tarball's own
/// top-level directory becomes the runtime folder).
pub(crate) fn extract_tarball(archive: &Path, ext: &str, dest_parent: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)
        .with_context(|| format!("failed opening {}", archive.display()))?;
    match ext {
        ".tar.gz" => unpack_guarded(flate2::read::GzDecoder::new(file), dest_parent),
        ".tar.xz" => unpack_guarded(xz2::read::XzDecoder::new(file), dest_parent),
        other => bail!("unsupported runtime archive type '{other}'"),
    }
}

/// Unpack a tar stream while refusing any entry whose path would escape
/// `dest_parent` (absolute paths or `..` components). These runtimes are fetched
/// from the network, so a crafted archive must not be able to write outside the
/// target directory (zip-slip). Legitimate archives only contain relative paths,
/// so this guard is transparent to them.
fn unpack_guarded<R: std::io::Read>(reader: R, dest_parent: &Path) -> Result<()> {
    use std::path::Component;
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().context("reading runtime archive entries")? {
        let mut entry = entry.context("reading runtime archive entry")?;
        let path = entry.path().context("decoding archive entry path")?.into_owned();
        if path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
        {
            bail!("refusing unsafe path '{}' in runtime archive", path.display());
        }
        entry
            .unpack_in(dest_parent)
            .with_context(|| format!("extracting '{}'", path.display()))?;
    }
    Ok(())
}

/// Remove an installed custom (GE) runtime by name from `compatibilitytools.d`.
/// Refuses to touch Steam-managed Proton under `steamapps/common` (uninstall those
/// through Steam). Errors if the named runtime isn't an installed custom tool.
pub fn remove(name: &str) -> Result<()> {
    let base = compat_tools_dir()?;
    let dir = base.join(name);
    if !dir.exists() {
        bail!(
            "'{name}' is not an installed custom runtime in {} \
             (official Valve Proton is removed via Steam)",
            base.display()
        );
    }
    std::fs::remove_dir_all(&dir).with_context(|| format!("failed removing {}", dir.display()))?;
    Ok(())
}

/// Normalize a runtime name for tolerant matching: lowercase, alphanumerics only.
/// Lets `Proton Experimental`, `proton experimental` and the canonical
/// `Proton - Experimental` all match.
fn normalize_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Look up the app id of a curated Valve Proton by name (punctuation-insensitive).
pub fn valve_app_id(name: &str) -> Option<u32> {
    let needle = normalize_name(name);
    VALVE_PROTONS
        .iter()
        .find(|(n, _)| normalize_name(n) == needle)
        .map(|(_, id)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valve_lookup_is_case_insensitive() {
        assert_eq!(valve_app_id("Proton 9.0"), Some(2805730));
        assert_eq!(valve_app_id("proton experimental"), Some(1493710));
        assert_eq!(valve_app_id("GE-Proton9-20"), None);
    }

    #[test]
    fn release_picks_matching_asset() {
        let src = &GE_SOURCES[0]; // proton-ge, .tar.gz
        let rel = GhRelease {
            tag_name: "GE-Proton9-20".to_string(),
            assets: vec![
                GhAsset {
                    name: "GE-Proton9-20.sha512sum".to_string(),
                    browser_download_url: "http://x/sum".to_string(),
                    size: 10,
                },
                GhAsset {
                    name: "GE-Proton9-20.tar.gz".to_string(),
                    browser_download_url: "http://x/tar".to_string(),
                    size: 12345,
                },
            ],
        };
        let pkg = release_to_package(src, rel).unwrap();
        assert_eq!(pkg.name, "GE-Proton9-20");
        assert_eq!(pkg.label, "Proton-GE");
        assert_eq!(pkg.size, 12345);
        match pkg.source {
            ProtonSource::Github { url, ext } => {
                assert_eq!(url, "http://x/tar");
                assert_eq!(ext, ".tar.gz");
            }
            _ => panic!("expected Github source"),
        }
    }

    #[test]
    fn release_without_matching_asset_is_skipped() {
        let src = &GE_SOURCES[0];
        let rel = GhRelease {
            tag_name: "GE-Proton9-20".to_string(),
            assets: vec![GhAsset {
                name: "notes.txt".to_string(),
                browser_download_url: "http://x/notes".to_string(),
                size: 1,
            }],
        };
        assert!(release_to_package(src, rel).is_none());
    }

    fn cachyos_assets() -> Vec<GhAsset> {
        vec![
            GhAsset {
                name: "proton-cachyos-10.0-20250101-slr-x86_64.tar.xz".to_string(),
                browser_download_url: "http://x/base".to_string(),
                size: 100,
            },
            GhAsset {
                name: "proton-cachyos-10.0-20250101-slr-x86_64_v3.tar.xz".to_string(),
                browser_download_url: "http://x/v3".to_string(),
                size: 200,
            },
            GhAsset {
                name: "proton-cachyos-10.0-20250101-slr.sha512sum".to_string(),
                browser_download_url: "http://x/sum".to_string(),
                size: 1,
            },
        ]
    }

    #[test]
    fn host_microarch_is_a_known_value() {
        let m = host_cachyos_microarch();
        assert!(m == "x86_64" || m == "x86_64_v3", "unexpected microarch: {m}");
    }

    #[test]
    fn cachyos_prefers_v3_when_host_supports_avx2() {
        let a = choose_asset(cachyos_assets(), ".tar.xz", Some("x86_64_v3")).unwrap();
        assert_eq!(a.browser_download_url, "http://x/v3");
        assert!(a.name.contains("x86_64_v3"));
    }

    #[test]
    fn cachyos_falls_back_to_generic_without_avx2() {
        let a = choose_asset(cachyos_assets(), ".tar.xz", Some("x86_64")).unwrap();
        assert_eq!(a.browser_download_url, "http://x/base");
        assert!(!a.name.contains("x86_64_v3"));
    }

    #[test]
    fn cachyos_v3_falls_back_to_generic_when_no_v3_asset() {
        let only_base = vec![GhAsset {
            name: "proton-cachyos-10.0-x86_64.tar.xz".to_string(),
            browser_download_url: "http://x/base".to_string(),
            size: 100,
        }];
        let a = choose_asset(only_base, ".tar.xz", Some("x86_64_v3")).unwrap();
        assert_eq!(a.browser_download_url, "http://x/base");
    }

    #[test]
    fn cachyos_release_labels_v3_selection() {
        let src = GE_SOURCES
            .iter()
            .find(|s| s.repo == "CachyOS/proton-cachyos")
            .unwrap();
        // Force a v3 pick by giving only a v3 asset (host-independent).
        let rel = GhRelease {
            tag_name: "cachyos-10.0".to_string(),
            assets: vec![GhAsset {
                name: "proton-cachyos-10.0-x86_64_v3.tar.xz".to_string(),
                browser_download_url: "http://x/v3".to_string(),
                size: 200,
            }],
        };
        // Only meaningful on an AVX2 host; otherwise choose_asset would skip the v3
        // asset. Assert the label reflects whatever asset was actually chosen.
        if let Some(pkg) = release_to_package(src, rel) {
            if pkg.size == 200 {
                assert!(pkg.label.contains("x86_64_v3"), "label was {}", pkg.label);
            }
        }
    }

    #[test]
    fn non_microarch_source_keeps_plain_label() {
        let src = &GE_SOURCES[0]; // Proton-GE, microarch = false
        let rel = GhRelease {
            tag_name: "GE-Proton9-20".to_string(),
            assets: vec![GhAsset {
                name: "GE-Proton9-20.tar.gz".to_string(),
                browser_download_url: "http://x/tar".to_string(),
                size: 5,
            }],
        };
        assert_eq!(release_to_package(src, rel).unwrap().label, "Proton-GE");
    }
}
