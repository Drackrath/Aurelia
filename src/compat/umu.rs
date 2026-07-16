//! umu-launcher plugin — on-the-fly download manager.
//!
//! [umu-launcher](https://github.com/Open-Wine-Components/umu-launcher) is a unified
//! launcher (`umu-run`) that runs Windows games through Proton **outside** of Steam,
//! selecting the Proton build via `PROTONPATH` and identifying the title via `GAMEID`.
//! Aurelia treats it as an **optional plugin**: unlike luxtorpeda (which *replaces* the
//! runner with a native engine), umu *wraps* Proton — the WineTkg/Proton runner still
//! resolves the Proton tree, but the game is spawned through `umu-run` instead of a bare
//! `proton run`. It is never bundled or linked in, only downloaded into Aurelia's own data
//! dir when the user enables the feature and a game is actually routed through it.
//!
//! The payload lives under `~/.config/Aurelia/plugins/umu` so it is self-contained and
//! removable.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// GitHub "latest release" endpoint for the umu-launcher project.
const RELEASE_API: &str =
    "https://api.github.com/repos/Open-Wine-Components/umu-launcher/releases/latest";

/// The executable Aurelia invokes from an extracted / configured umu install.
const ENTRY_NAME: &str = "umu-run";

/// A umu install discovered on disk.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledUmu {
    /// The release tag that was installed (from the stamped version file).
    pub version: String,
    /// The install root (the directory containing `umu-run`).
    pub root: PathBuf,
    /// The executable Aurelia invokes (`<root>/umu-run`).
    pub entry: PathBuf,
}

/// A release asset selected for download.
#[derive(Debug, Clone)]
struct UmuRelease {
    tag: String,
    url: String,
    ext: String,
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// The directory Aurelia extracts the umu payload into.
pub fn plugin_dir() -> Result<PathBuf> {
    Ok(crate::core::config::config_dir()?.join("plugins").join("umu"))
}

/// Path of the file we stamp with the installed release tag.
fn version_stamp(base: &Path) -> PathBuf {
    base.join(".aurelia_version")
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Find the install root under `base`: `base` itself if it holds an `umu-run`,
/// otherwise the first immediate subdirectory that does (the tarball's own top dir).
fn find_entry_root(base: &Path) -> Option<PathBuf> {
    if base.join(ENTRY_NAME).is_file() {
        return Some(base.to_path_buf());
    }
    let entries = std::fs::read_dir(base).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() && p.join(ENTRY_NAME).is_file() {
            return Some(p);
        }
    }
    None
}

/// The executable to invoke for an install rooted at `root`.
pub fn entry_point(root: &Path) -> PathBuf {
    root.join(ENTRY_NAME)
}

/// Return the install in use, if any. A configured `custom` path (an externally-managed
/// umu) takes precedence over Aurelia's managed plugin directory. A custom path may point
/// at a directory containing `umu-run` **or** directly at a `umu-run` binary.
pub fn installed(custom: Option<&Path>) -> Option<InstalledUmu> {
    if let Some(custom) = custom {
        // A custom path may be the umu-run binary itself, or a directory holding it.
        if custom.is_file() && custom.file_name().and_then(|n| n.to_str()) == Some(ENTRY_NAME) {
            let root = custom.parent().map(Path::to_path_buf).unwrap_or_else(|| custom.to_path_buf());
            return Some(InstalledUmu {
                version: "custom".to_string(),
                entry: custom.to_path_buf(),
                root,
            });
        }
        let root = find_entry_root(custom)?;
        return Some(InstalledUmu {
            version: "custom".to_string(),
            entry: entry_point(&root),
            root,
        });
    }
    let base = plugin_dir().ok()?;
    let root = find_entry_root(&base)?;
    let version = std::fs::read_to_string(version_stamp(&base))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    Some(InstalledUmu {
        version,
        entry: entry_point(&root),
        root,
    })
}

// ---------------------------------------------------------------------------
// Release lookup
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Query GitHub for the latest umu-launcher release and pick its tarball asset.
async fn latest_release() -> Result<UmuRelease> {
    let client = reqwest::Client::builder()
        .user_agent("aurelia-umu-plugin")
        .build()
        .context("failed to build the GitHub HTTP client")?;

    let release: GithubRelease = client
        .get(RELEASE_API)
        .send()
        .await
        .context("failed requesting the umu-launcher latest release")?
        .error_for_status()
        .context("GitHub returned an error for the umu-launcher latest release")?
        .json()
        .await
        .context("failed parsing the umu-launcher release JSON")?;

    // Prefer a .tar.gz, then .tar.xz; skip checksum sidecars (.sha*).
    let pick = |ext: &str| {
        release
            .assets
            .iter()
            .find(|a| a.name.ends_with(ext) && !a.name.contains(".sha"))
    };
    let (asset, ext) = pick(".tar.gz")
        .map(|a| (a, ".tar.gz"))
        .or_else(|| pick(".tar.xz").map(|a| (a, ".tar.xz")))
        .with_context(|| {
            format!(
                "no .tar.gz/.tar.xz asset on umu-launcher release '{}'",
                release.tag_name
            )
        })?;

    Ok(UmuRelease {
        tag: release.tag_name.clone(),
        url: asset.browser_download_url.clone(),
        ext: ext.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Install / update / remove
// ---------------------------------------------------------------------------

/// Download the latest umu-launcher release and extract it into the plugin directory,
/// replacing any previous payload. Returns the resolved `umu-run` path.
pub async fn install(on_progress: &mut (dyn FnMut(u64, u64) + Send)) -> Result<PathBuf> {
    let release = latest_release().await?;
    let base = plugin_dir()?;

    // Start clean so a stale layout can't shadow the new one.
    if base.exists() {
        std::fs::remove_dir_all(&base)
            .with_context(|| format!("failed clearing {}", base.display()))?;
    }
    std::fs::create_dir_all(&base)
        .with_context(|| format!("failed creating {}", base.display()))?;

    let tmp = base.join(format!(".download{}", release.ext));
    crate::compat::proton::download_to(&release.url, &tmp, on_progress)
        .await
        .context("failed downloading umu-launcher")?;

    let result = crate::compat::proton::extract_tarball(&tmp, &release.ext, &base)
        .context("failed extracting umu-launcher");
    let _ = std::fs::remove_file(&tmp);
    result?;

    std::fs::write(version_stamp(&base), &release.tag)
        .with_context(|| format!("failed stamping version in {}", base.display()))?;

    let root = find_entry_root(&base)
        .context("umu-launcher archive did not contain a `umu-run` executable")?;
    Ok(entry_point(&root))
}

/// Resolve a usable `umu-run` path for launching.
///
/// When `custom` is set, that externally-managed install is used as-is and **nothing is
/// ever downloaded** (an error is returned if no `umu-run` is found). Otherwise the managed
/// plugin is used, downloading it on first use.
pub async fn ensure_installed(custom: Option<&Path>) -> Result<PathBuf> {
    if let Some(custom) = custom {
        return installed(Some(custom))
            .map(|inst| inst.entry)
            .with_context(|| {
                format!(
                    "configured umu_path '{}' does not contain a `umu-run` executable",
                    custom.display()
                )
            });
    }
    if let Some(inst) = installed(None) {
        return Ok(inst.entry);
    }
    let mut noop = |_, _| {};
    install(&mut noop).await
}

/// Remove the umu payload from disk. Returns `false` if nothing was installed.
pub fn uninstall() -> Result<bool> {
    let base = plugin_dir()?;
    if !base.exists() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&base)
        .with_context(|| format!("failed removing {}", base.display()))?;
    Ok(true)
}

#[cfg(test)]
#[path = "umu_tests.rs"]
mod tests;
