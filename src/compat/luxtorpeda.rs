//! Luxtorpeda native-engine plugin — on-the-fly download manager.
//!
//! [Luxtorpeda](https://codeberg.org/luxtorpeda/luxtorpeda) is a standalone Steam Play
//! compatibility tool (GPL-2.0) that runs games on native Linux engines instead of
//! Proton/Wine. Aurelia treats it as an **optional plugin**: it is never bundled or linked
//! in, only downloaded into Aurelia's own data dir when the user enables the feature and a
//! game is actually routed through it, then invoked over a process boundary (exactly how
//! Steam invokes a compatibility tool).
//!
//! The payload lives under `~/.config/Aurelia/plugins/luxtorpeda` so it is self-contained
//! and removable, independent of Steam's `compatibilitytools.d`.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Codeberg (Gitea) "latest release" endpoint for the luxtorpeda client.
const RELEASE_API: &str =
    "https://codeberg.org/api/v1/repos/luxtorpeda/luxtorpeda/releases/latest";

/// A luxtorpeda install discovered on disk.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledLux {
    /// The release tag that was installed (from the stamped version file).
    pub version: String,
    /// The tool root (the directory containing `toolmanifest.vdf`).
    pub root: PathBuf,
    /// The executable Aurelia invokes (`<root>/luxtorpeda` unless the manifest says otherwise).
    pub entry: PathBuf,
}

/// A release asset selected for download.
#[derive(Debug, Clone)]
struct LuxRelease {
    tag: String,
    url: String,
    ext: String,
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// The directory Aurelia extracts the luxtorpeda payload into.
pub fn plugin_dir() -> Result<PathBuf> {
    Ok(crate::core::config::config_dir()?.join("plugins").join("luxtorpeda"))
}

/// Path of the file we stamp with the installed release tag.
fn version_stamp(base: &Path) -> PathBuf {
    base.join(".aurelia_version")
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Find the tool root under `base`: `base` itself if it holds a `toolmanifest.vdf`,
/// otherwise the first immediate subdirectory that does (the tarball's own top dir).
fn find_tool_root(base: &Path) -> Option<PathBuf> {
    if base.join("toolmanifest.vdf").exists() {
        return Some(base.to_path_buf());
    }
    let entries = std::fs::read_dir(base).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() && p.join("toolmanifest.vdf").exists() {
            return Some(p);
        }
    }
    None
}

/// Resolve the executable to invoke. Parses the `commandline` value from
/// `toolmanifest.vdf` (the first whitespace token, e.g. `/luxtorpeda`, is a path
/// relative to the tool root); falls back to `<root>/luxtorpeda`.
pub fn entry_point(root: &Path) -> PathBuf {
    let fallback = root.join("luxtorpeda");
    let Ok(manifest) = std::fs::read_to_string(root.join("toolmanifest.vdf")) else {
        return fallback;
    };
    parse_commandline(&manifest)
        .map(|rel| root.join(rel.trim_start_matches('/')))
        .unwrap_or(fallback)
}

/// Extract the first token of the `"commandline"` value from a `toolmanifest.vdf`
/// body, e.g. `"commandline"  "/luxtorpeda %verb%"` -> `/luxtorpeda`.
fn parse_commandline(manifest: &str) -> Option<String> {
    let idx = manifest.find("\"commandline\"")?;
    let rest = &manifest[idx + "\"commandline\"".len()..];
    let start = rest.find('"')? + 1;
    let end = rest[start..].find('"')? + start;
    let value = &rest[start..end];
    value.split_whitespace().next().map(str::to_string)
}

/// Return the install in use, if any. A configured `custom` path (an externally-managed
/// luxtorpeda) takes precedence over Aurelia's managed plugin directory.
pub fn installed(custom: Option<&Path>) -> Option<InstalledLux> {
    if let Some(custom) = custom {
        let root = find_tool_root(custom)?;
        return Some(InstalledLux {
            version: "custom".to_string(),
            entry: entry_point(&root),
            root,
        });
    }
    let base = plugin_dir().ok()?;
    let root = find_tool_root(&base)?;
    let version = std::fs::read_to_string(version_stamp(&base))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    Some(InstalledLux {
        version,
        entry: entry_point(&root),
        root,
    })
}

// ---------------------------------------------------------------------------
// Release lookup
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GiteaRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GiteaAsset>,
}

#[derive(Debug, Deserialize)]
struct GiteaAsset {
    name: String,
    browser_download_url: String,
}

/// Query Codeberg for the latest luxtorpeda client release and pick its tarball asset.
async fn latest_release() -> Result<LuxRelease> {
    let client = reqwest::Client::builder()
        .user_agent("aurelia-luxtorpeda-plugin")
        .build()
        .context("failed to build the Codeberg HTTP client")?;

    let release: GiteaRelease = client
        .get(RELEASE_API)
        .send()
        .await
        .context("failed requesting the luxtorpeda latest release")?
        .error_for_status()
        .context("Codeberg returned an error for the luxtorpeda latest release")?
        .json()
        .await
        .context("failed parsing the luxtorpeda release JSON")?;

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
                "no .tar.gz/.tar.xz asset on luxtorpeda release '{}'",
                release.tag_name
            )
        })?;

    Ok(LuxRelease {
        tag: release.tag_name.clone(),
        url: asset.browser_download_url.clone(),
        ext: ext.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Install / update / remove
// ---------------------------------------------------------------------------

/// Download the latest luxtorpeda release and extract it into the plugin directory,
/// replacing any previous payload. Returns the resolved entry point.
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
        .context("failed downloading luxtorpeda")?;

    let result = crate::compat::proton::extract_tarball(&tmp, &release.ext, &base)
        .context("failed extracting luxtorpeda");
    let _ = std::fs::remove_file(&tmp);
    result?;

    std::fs::write(version_stamp(&base), &release.tag)
        .with_context(|| format!("failed stamping version in {}", base.display()))?;

    let root = find_tool_root(&base)
        .context("luxtorpeda archive did not contain a toolmanifest.vdf")?;
    Ok(entry_point(&root))
}

/// Resolve a usable luxtorpeda entry point for launching.
///
/// When `custom` is set, that externally-managed install is used as-is and **nothing is
/// ever downloaded** (an error is returned if it has no `toolmanifest.vdf`). Otherwise the
/// managed plugin is used, downloading it on first use.
pub async fn ensure_installed(custom: Option<&Path>) -> Result<PathBuf> {
    if let Some(custom) = custom {
        let root = find_tool_root(custom).with_context(|| {
            format!(
                "configured luxtorpeda_path '{}' does not contain a toolmanifest.vdf",
                custom.display()
            )
        })?;
        return Ok(entry_point(&root));
    }
    if let Some(inst) = installed(None) {
        return Ok(inst.entry);
    }
    let mut noop = |_, _| {};
    install(&mut noop).await
}

/// Remove the luxtorpeda payload from disk. Returns `false` if nothing was installed.
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
mod tests {
    use super::*;

    #[test]
    fn parses_commandline_path() {
        let manifest = "\"manifest\"\n{\n  \"commandline\" \"/luxtorpeda %verb%\"\n}\n";
        assert_eq!(parse_commandline(manifest).as_deref(), Some("/luxtorpeda"));
    }

    #[test]
    fn parses_commandline_extra_spacing() {
        let manifest = "\"commandline\"    \"/bin/luxtorpeda   %verb%\"";
        assert_eq!(parse_commandline(manifest).as_deref(), Some("/bin/luxtorpeda"));
    }

    #[test]
    fn missing_commandline_is_none() {
        assert_eq!(parse_commandline("\"manifest\" { }"), None);
    }

    #[test]
    fn selects_tarball_over_checksum() {
        // Mirrors the asset-picking logic in `latest_release` without the network.
        let assets = [
            ("luxtorpeda.tar.gz.sha256", false),
            ("luxtorpeda.tar.gz", true),
        ];
        let chosen = assets
            .iter()
            .find(|(name, _)| name.ends_with(".tar.gz") && !name.contains(".sha"));
        assert_eq!(chosen.map(|(_, ok)| *ok), Some(true));
    }
}
