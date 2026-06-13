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
//! Proton lives in [`crate::config::LauncherConfig::proton_version`]; a fresh install
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
}

const GE_SOURCES: &[GithubSource] = &[
    GithubSource { repo: "GloriousEggroll/proton-ge-custom", label: "Proton-GE", ext: ".tar.gz" },
    GithubSource { repo: "GloriousEggroll/wine-ge-custom", label: "Wine-GE", ext: ".tar.xz" },
];

/// How many recent releases to list per GE repo.
const GE_RELEASES_PER_REPO: usize = 8;

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Steam's `compatibilitytools.d` directory, where custom (GE) runtimes live.
/// Derived from the detected Steam root, falling back to the conventional path.
pub fn compat_tools_dir() -> Result<PathBuf> {
    if let Some(steam) = crate::config::detect_steam_path() {
        return Ok(steam.join("compatibilitytools.d"));
    }
    let home = crate::config::home_dir()?;
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

    out.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// List everything installable: the curated Valve Proton set plus recent GE
/// releases fetched from GitHub. GitHub failures (offline, rate-limited) are logged
/// and that source is skipped rather than failing the whole listing.
pub async fn list_available() -> Result<Vec<ProtonPackage>> {
    let mut out: Vec<ProtonPackage> = VALVE_PROTONS
        .iter()
        .map(|(name, app_id)| ProtonPackage {
            name: name.to_string(),
            label: "Valve".to_string(),
            size: 0,
            source: ProtonSource::Valve { app_id: *app_id },
        })
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
        return Ok(ProtonPackage {
            name: vname.to_string(),
            label: "Valve".to_string(),
            size: 0,
            source: ProtonSource::Valve { app_id: *app_id },
        });
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

/// Map a GitHub release to a package, picking the first asset with the source's
/// extension (skipping checksum/`.sha*` files). Returns `None` if no asset matches.
fn release_to_package(src: &GithubSource, rel: GhRelease) -> Option<ProtonPackage> {
    let asset = rel
        .assets
        .into_iter()
        .find(|a| a.name.ends_with(src.ext))?;
    Some(ProtonPackage {
        name: rel.tag_name,
        label: src.label.to_string(),
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
async fn download_to(
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
fn extract_tarball(archive: &Path, ext: &str, dest_parent: &Path) -> Result<()> {
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
    let dir = compat_tools_dir()?.join(name);
    if !dir.exists() {
        bail!(
            "'{name}' is not an installed custom runtime in {} \
             (official Valve Proton is removed via Steam)",
            compat_tools_dir()?.display()
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
}
