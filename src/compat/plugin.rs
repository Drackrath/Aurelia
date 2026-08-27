//! Shared machinery for optional downloaded plugins (luxtorpeda, umu-launcher).
//!
//! Both plugins follow the same lifecycle: a payload downloaded from a forge's
//! "latest release" endpoint into `~/.config/Aurelia/plugins/<id>`, discovered
//! on disk via a marker file, stamped with the installed tag, and removable as
//! a directory. Everything but the marker/entry-point specifics lives here.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// A plugin install discovered on disk.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledPlugin {
    /// The release tag that was installed (from the stamped version file),
    /// `"custom"` for an externally-managed install.
    pub version: String,
    /// The install root (the directory holding the plugin's marker file).
    pub root: PathBuf,
    /// The executable Aurelia invokes.
    pub entry: PathBuf,
}

/// The per-plugin specifics the shared download manager is parameterized by.
pub(crate) struct PluginSpec {
    /// Directory name under `plugins/`.
    pub id: &'static str,
    /// Human-facing project name used in messages ("luxtorpeda", "umu-launcher").
    pub display: &'static str,
    /// Forge name used in messages ("Codeberg", "GitHub").
    pub host: &'static str,
    /// "latest release" API endpoint (Gitea and GitHub share the JSON shape).
    pub release_api: &'static str,
    /// HTTP User-Agent for the release lookup.
    pub user_agent: &'static str,
    /// Whether `path` is an install root (holds the plugin's marker file).
    pub root_marker: fn(&Path) -> bool,
    /// Error text when a freshly extracted archive has no install root.
    pub archive_marker_missing: &'static str,
}

/// A release asset selected for download.
struct PluginRelease {
    tag: String,
    url: String,
    ext: String,
}

/// The directory Aurelia extracts the plugin payload into.
pub(crate) fn plugin_dir(spec: &PluginSpec) -> Result<PathBuf> {
    Ok(crate::core::config::config_dir()?.join("plugins").join(spec.id))
}

/// Path of the file we stamp with the installed release tag.
fn version_stamp(base: &Path) -> PathBuf {
    base.join(".aurelia_version")
}

/// Find the install root under `base`: `base` itself if it holds the marker,
/// otherwise the first immediate subdirectory that does (the tarball's own top
/// dir).
pub(crate) fn find_root(spec: &PluginSpec, base: &Path) -> Option<PathBuf> {
    if (spec.root_marker)(base) {
        return Some(base.to_path_buf());
    }
    let entries = std::fs::read_dir(base).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() && (spec.root_marker)(&p) {
            return Some(p);
        }
    }
    None
}

/// The managed install under [`plugin_dir`], as `(version, root)`, if present.
pub(crate) fn managed_install(spec: &PluginSpec) -> Option<(String, PathBuf)> {
    let base = plugin_dir(spec).ok()?;
    let root = find_root(spec, &base)?;
    let version = std::fs::read_to_string(version_stamp(&base))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    Some((version, root))
}

/// Query the forge for the latest release and pick its tarball asset.
async fn latest_release(spec: &PluginSpec) -> Result<PluginRelease> {
    let client = reqwest::Client::builder()
        .user_agent(spec.user_agent)
        .build()
        .with_context(|| format!("failed to build the {} HTTP client", spec.host))?;

    // Gitea and GitHub releases share this JSON shape.
    let release: crate::compat::proton::GhRelease = client
        .get(spec.release_api)
        .send()
        .await
        .with_context(|| format!("failed requesting the {} latest release", spec.display))?
        .error_for_status()
        .with_context(|| {
            format!("{} returned an error for the {} latest release", spec.host, spec.display)
        })?
        .json()
        .await
        .with_context(|| format!("failed parsing the {} release JSON", spec.display))?;

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
                "no .tar.gz/.tar.xz asset on {} release '{}'",
                spec.display, release.tag_name
            )
        })?;

    Ok(PluginRelease {
        tag: release.tag_name.clone(),
        url: asset.browser_download_url.clone(),
        ext: ext.to_string(),
    })
}

/// Download the latest release and extract it into the plugin directory,
/// replacing any previous payload. Returns the extracted install root.
pub(crate) async fn install_payload(
    spec: &PluginSpec,
    on_progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Result<PathBuf> {
    let release = latest_release(spec).await?;
    let base = plugin_dir(spec)?;

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
        .with_context(|| format!("failed downloading {}", spec.display))?;

    let result = crate::compat::proton::extract_tarball(&tmp, &release.ext, &base)
        .with_context(|| format!("failed extracting {}", spec.display));
    let _ = std::fs::remove_file(&tmp);
    result?;

    std::fs::write(version_stamp(&base), &release.tag)
        .with_context(|| format!("failed stamping version in {}", base.display()))?;

    find_root(spec, &base).context(spec.archive_marker_missing)
}

/// Remove the plugin payload from disk. Returns `false` if nothing was installed.
pub(crate) fn uninstall(spec: &PluginSpec) -> Result<bool> {
    let base = plugin_dir(spec)?;
    if !base.exists() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&base)
        .with_context(|| format!("failed removing {}", base.display()))?;
    Ok(true)
}
