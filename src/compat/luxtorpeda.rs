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
//! Lifecycle shared via [`crate::compat::plugin`].

use crate::compat::plugin::{self, InstalledPlugin, PluginSpec};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The shared-plugin description of luxtorpeda.
static SPEC: PluginSpec = PluginSpec {
    id: "luxtorpeda",
    display: "luxtorpeda",
    host: "Codeberg",
    release_api: "https://codeberg.org/api/v1/repos/luxtorpeda/luxtorpeda/releases/latest",
    user_agent: "aurelia-luxtorpeda-plugin",
    root_marker: |p| p.join("toolmanifest.vdf").exists(),
    archive_marker_missing: "luxtorpeda archive did not contain a toolmanifest.vdf",
};

/// Find the tool root under `base`: `base` itself if it holds a `toolmanifest.vdf`,
/// otherwise the first immediate subdirectory that does (the tarball's own top dir).
fn find_tool_root(base: &Path) -> Option<PathBuf> {
    plugin::find_root(&SPEC, base)
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
pub fn installed(custom: Option<&Path>) -> Option<InstalledPlugin> {
    if let Some(custom) = custom {
        let root = find_tool_root(custom)?;
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

/// Download the latest luxtorpeda release and extract it into the plugin directory,
/// replacing any previous payload. Returns the resolved entry point.
pub async fn install(on_progress: &mut (dyn FnMut(u64, u64) + Send)) -> Result<PathBuf> {
    let root = plugin::install_payload(&SPEC, on_progress).await?;
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
    plugin::uninstall(&SPEC)
}

#[cfg(test)]
#[path = "luxtorpeda_tests.rs"]
mod tests;
