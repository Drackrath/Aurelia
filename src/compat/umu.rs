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
//! removable. The download/discovery lifecycle is shared with the other plugins (see
//! [`crate::compat::plugin`]).

use crate::compat::plugin::{self, InstalledPlugin, PluginSpec};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The executable Aurelia invokes from an extracted / configured umu install.
const ENTRY_NAME: &str = "umu-run";

/// The shared-plugin description of umu-launcher.
static SPEC: PluginSpec = PluginSpec {
    id: "umu",
    display: "umu-launcher",
    host: "GitHub",
    release_api: "https://api.github.com/repos/Open-Wine-Components/umu-launcher/releases/latest",
    user_agent: "aurelia-umu-plugin",
    root_marker: |p| p.join(ENTRY_NAME).is_file(),
    archive_marker_missing: "umu-launcher archive did not contain a `umu-run` executable",
};

/// The directory Aurelia extracts the umu payload into.
pub fn plugin_dir() -> Result<PathBuf> {
    plugin::plugin_dir(&SPEC)
}

/// Find the install root under `base`: `base` itself if it holds an `umu-run`,
/// otherwise the first immediate subdirectory that does (the tarball's own top dir).
fn find_entry_root(base: &Path) -> Option<PathBuf> {
    plugin::find_root(&SPEC, base)
}

/// The executable to invoke for an install rooted at `root`.
pub fn entry_point(root: &Path) -> PathBuf {
    root.join(ENTRY_NAME)
}

/// Return the install in use, if any. A configured `custom` path (an externally-managed
/// umu) takes precedence over Aurelia's managed plugin directory. A custom path may point
/// at a directory containing `umu-run` **or** directly at a `umu-run` binary.
pub fn installed(custom: Option<&Path>) -> Option<InstalledPlugin> {
    if let Some(custom) = custom {
        // A custom path may be the umu-run binary itself, or a directory holding it.
        if custom.is_file() && custom.file_name().and_then(|n| n.to_str()) == Some(ENTRY_NAME) {
            let root = custom.parent().map(Path::to_path_buf).unwrap_or_else(|| custom.to_path_buf());
            return Some(InstalledPlugin {
                version: "custom".to_string(),
                entry: custom.to_path_buf(),
                root,
            });
        }
        let root = find_entry_root(custom)?;
        return Some(InstalledPlugin {
            version: "custom".to_string(),
            entry: entry_point(&root),
            root,
        });
    }
    let (version, root) = plugin::managed_install(&SPEC)?;
    Some(InstalledPlugin {
        version,
        entry: entry_point(&root),
        root,
    })
}

/// Download the latest umu-launcher release and extract it into the plugin directory,
/// replacing any previous payload. Returns the resolved `umu-run` path.
pub async fn install(on_progress: &mut (dyn FnMut(u64, u64) + Send)) -> Result<PathBuf> {
    let root = plugin::install_payload(&SPEC, on_progress).await?;
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
    plugin::uninstall(&SPEC)
}

#[cfg(test)]
#[path = "umu_tests.rs"]
mod tests;
