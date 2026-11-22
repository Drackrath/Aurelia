//! Runner resolution, classification and commands.

use anyhow::{Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn build_runner_command(runner_path: &Path) -> Result<Command> {
    let mut final_path = runner_path.to_path_buf();

    // 1. Directory Resolution: If it's a directory, find the binary
    if final_path.is_dir() {
        if final_path.join("proton").exists() {
            final_path.push("proton");
        } else if final_path.join("bin/wine").exists() {
            final_path.push("bin/wine");
        } else if final_path.join("bin/wine64").exists() {
            final_path.push("bin/wine64");
        }
    }

    // 2. Identification and Command Building
    if let Some(file_name) = final_path.file_name().and_then(|f| f.to_str()) {
        if file_name == "proton" {
            let mut cmd = Command::new(&final_path);
            cmd.arg("run");
            return Ok(cmd);
        }
        if file_name == "wine" || file_name == "wine64" {
            return Ok(Command::new(&final_path));
        }
    }

    // 3. Last Resort: Just return the command if it exists
    if final_path.exists() && final_path.is_file() {
        return Ok(Command::new(&final_path));
    }

    bail!(
        "Failed to resolve a valid runner binary from {}",
        runner_path.display()
    )
}

/// Normalize a runtime name for fuzzy matching: lowercase, alphanumerics only.
/// So `experimental`, `Proton Experimental` and the on-disk `Proton - Experimental`
/// all reduce to comparable forms.
fn normalize_runner_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Resolve a Proton/Wine runtime *name* (e.g. `Proton 9.0`, `GE-Proton9-20`, the
/// legacy default `experimental`) or path to an on-disk runtime directory/binary.
///
/// Looks under the Steam library's `steamapps/common`, `compatibilitytools.d`
/// (both the configured library and the detected Steam root), and Lutris. Falls
/// back to a normalized fuzzy match so a configured name like `experimental`
/// still finds Steam's `Proton - Experimental` directory. Returns the name as a
/// path if nothing matches (the caller surfaces a clear error).
pub fn resolve_runner(name: &str, library_root: &Path) -> PathBuf {
    resolve_runner_opt(name, library_root).unwrap_or_else(|| {
        // Fallback: return the name verbatim as a path so the caller surfaces a clear
        // "binary not found" error. The warn lives here (not in resolve_runner_opt) so
        // callers that only want to *probe* resolvability can do so quietly.
        tracing::warn!(
            runner = %name,
            "could not resolve runner '{}' to an on-disk runtime under any known search path; \
             returning the name verbatim as a path (the launch will likely fail resolving the binary)",
            name
        );
        Path::new(name).to_path_buf()
    })
}

/// The resolution core behind [`resolve_runner`], returning `None` (quietly) when the
/// name matches no on-disk runtime. Use this when you want to *check* whether a runner
/// resolves without emitting the not-found warning [`resolve_runner`] logs.
pub fn resolve_runner_opt(name: &str, library_root: &Path) -> Option<PathBuf> {
    resolve_runner_opt_in(name, library_root, &crate::library::steam_root_candidates())
}

/// Configured library first, then all others.
pub fn runner_library_roots(configured: &Path, steam_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut libraries = vec![configured.to_path_buf()];
    libraries.extend(steam_roots.iter().cloned());
    let mut roots = vec![configured.to_path_buf()];
    roots.extend(steam_roots.iter().cloned());
    for root in &roots {
        for vdf in [
            root.join("steamapps").join("libraryfolders.vdf"),
            root.join("config").join("libraryfolders.vdf"),
        ] {
            if let Ok(raw) = std::fs::read_to_string(&vdf) {
                libraries.extend(crate::library::library_folder_paths(&raw));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    libraries.retain(|p| seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())));
    libraries
}

/// Testable core of [`resolve_runner_opt`].
pub fn resolve_runner_opt_in(
    name: &str,
    library_root: &Path,
    steam_roots: &[PathBuf],
) -> Option<PathBuf> {
    let name_path = Path::new(name);
    if name_path.is_absolute() || name_path.exists() {
        return Some(name_path.to_path_buf());
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let libraries = runner_library_roots(library_root, steam_roots);
    // Configured library wins ties.
    let mut search_dirs: Vec<PathBuf> = vec![
        library_root.join("steamapps/common"),
        library_root.join("compatibilitytools.d"),
    ];
    for lib in libraries.iter().skip(1) {
        search_dirs.push(lib.join("steamapps/common"));
    }
    for root in steam_roots {
        search_dirs.push(root.join("compatibilitytools.d"));
    }
    // The Steam root's compatibilitytools.d (handles a non-standard / Flatpak Steam
    // whose root differs from `library_root`).
    if let Ok(compat) = crate::compat::proton::compat_tools_dir() {
        search_dirs.push(compat);
    }
    search_dirs.push(PathBuf::from(&home).join(".local/share/lutris/runners/wine"));

    // 1. Exact directory-name match (fast path: "Proton 9.0", "GE-Proton9-20").
    for dir in &search_dirs {
        let exact = dir.join(name);
        if exact.exists() {
            return Some(exact);
        }
    }

    // 2. Fuzzy match on normalized names ("experimental" → "Proton - Experimental").
    let needle = normalize_runner_name(name);
    if !needle.is_empty() {
        let mut matches: Vec<PathBuf> = Vec::new();
        for dir in &search_dirs {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    if !entry.path().is_dir() {
                        continue;
                    }
                    let cand = normalize_runner_name(&entry.file_name().to_string_lossy());
                    if !cand.is_empty()
                        && (cand == needle || cand.contains(&needle) || needle.contains(&cand))
                    {
                        matches.push(entry.path());
                    }
                }
            }
        }
        // Prefer an exact normalized match; otherwise the shortest (closest) name.
        matches.sort_by_key(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().len())
                .unwrap_or(usize::MAX)
        });
        if let Some(exact) = matches.iter().find(|p| {
            normalize_runner_name(&p.file_name().unwrap_or_default().to_string_lossy()) == needle
        }) {
            return Some(exact.clone());
        }
        if let Some(first) = matches.into_iter().next() {
            return Some(first);
        }
    }

    None
}

/// Coarse classification of a runner directory tree, used to decide how a process
/// must be invoked. See [`classify_runner`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RunnerKind {
    /// A Proton tree: has a `proton` python launch script at the root and/or the
    /// Proton `files/bin/wine` layout. The game is launched through `proton run`
    /// (so protonfixes apply), but a bare wine binary for background Steam must be
    /// taken from `files/bin/wine64` directly — never `proton run`.
    Proton,
    /// A wine-tkg build: `bin/wine`(64) present, no `proton` script, name marks tkg.
    WineTkg,
    /// A plain upstream/custom Wine build: `bin/wine`(64) present, no `proton` script.
    PlainWine,
    /// None of the above — not a usable runner tree.
    Unknown,
}

/// Classify a runner directory by inspecting its on-disk layout.
///
/// Detection order matters: a Proton tree is identified first (it owns a `proton`
/// launch script and/or the `files/bin/wine` layout), then bare Wine trees
/// (`bin/wine`/`bin/wine64` with no `proton` script), split into wine-tkg vs plain
/// Wine by a name heuristic.
pub(crate) fn classify_runner(runner_dir: &Path) -> RunnerKind {
    // Proton: the `proton` launch script at the root is the canonical marker (it is
    // what `build_runner_command` probes to append `run`). The Proton `files/bin/wine`
    // layout is an additional signal.
    if runner_dir.join("proton").exists()
        || runner_dir.join("files/bin/wine").exists()
        || runner_dir.join("files/bin/wine64").exists()
    {
        return RunnerKind::Proton;
    }

    // Bare Wine tree: wine binaries under `bin/` with no Proton wrapper.
    if runner_dir.join("bin/wine").exists() || runner_dir.join("bin/wine64").exists() {
        let name = runner_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if name.contains("tkg") {
            return RunnerKind::WineTkg;
        }
        return RunnerKind::PlainWine;
    }

    RunnerKind::Unknown
}

/// Locate a Proton tree's bundled bare wine binary (prefer 64-bit).
///
/// Background Steam must run under a bare wine, not the `proton run` protonfixes
/// wrapper. When the only runtime available is a Proton tree, this yields the wine
/// binary shipped inside it (`files/bin/wine64`, then a few known fallbacks).
pub(crate) fn proton_bundled_bare_wine(runner_dir: &Path) -> Option<PathBuf> {
    let root = derive_runner_root(runner_dir);
    for rel in [
        "files/bin/wine64",
        "files/bin/wine",
        "dist/bin/wine64",
        "dist/bin/wine",
    ] {
        let p = root.join(rel);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Validate that `path` is a usable bare wine / plain-wine runner suitable for
/// hosting the background Steam process.
///
/// Background Steam must NOT be hosted by a `proton run` wrapper — it needs a bare
/// wine. This accepts a wine/wine64 binary directly, or a wine-tkg / plain-Wine
/// directory tree; it rejects a missing path, a `proton` script/tree, and anything
/// unrecognised.
pub(crate) fn validate_steam_runtime_runner_path(path: &Path) -> Result<()> {
    if !path.exists() {
        bail!(
            "Steam-runtime runner path does not exist: {}",
            path.display()
        );
    }

    // A direct binary is fine only if it is a bare wine/wine64 (not the `proton` script).
    if path.is_file() {
        match path.file_name().and_then(|f| f.to_str()) {
            Some("wine") | Some("wine64") => return Ok(()),
            Some("proton") => bail!(
                "Steam-runtime runner points at a `proton` launch script ({}); background \
                 Steam requires a bare wine binary, not a `proton run` wrapper",
                path.display()
            ),
            _ => bail!(
                "Steam-runtime runner binary is not a wine/wine64 executable: {}",
                path.display()
            ),
        }
    }

    match classify_runner(path) {
        RunnerKind::WineTkg | RunnerKind::PlainWine => Ok(()),
        RunnerKind::Proton => bail!(
            "Steam-runtime runner resolves to a Proton tree ({}); background Steam must run \
             under a bare wine, not the `proton run` protonfixes wrapper. Point \
             `steam_runtime_runner` at a wine-tkg or plain-Wine build",
            path.display()
        ),
        RunnerKind::Unknown => bail!(
            "Steam-runtime runner path is not a usable wine runner (no bin/wine[64]): {}",
            path.display()
        ),
    }
}

/// Whether this build can host the self-contained Windows Steam runtime. It runs
/// `steam.exe` under a Proton/Wine runner, and those ship Linux (ELF) binaries:
/// Windows rejects them with OS error 193 ("not a valid Win32 application").
pub const STEAM_RUNTIME_SUPPORTED: bool = cfg!(target_os = "linux");

/// Why the Windows Steam runtime is unavailable on this platform, with the
/// alternative to use instead.
pub fn steam_runtime_unsupported_msg() -> String {
    let alternative = if cfg!(target_os = "windows") {
        "On Windows Aurelia uses your installed Steam client instead: install Steam and \
         run games with `aurelia play <APP_ID> --steam`."
    } else {
        "Run Aurelia on Linux to use it."
    };
    format!(
        "the self-contained Windows Steam runtime is only supported on Linux: it runs \
         Steam under a Proton/Wine runner, whose binaries are Linux executables and \
         cannot be started on {}. {alternative}",
        std::env::consts::OS
    )
}

/// Whether Proton/Wine runners, and the plugins built on them (umu, luxtorpeda),
/// can run on this platform. Same reason as [`STEAM_RUNTIME_SUPPORTED`].
pub const PROTON_SUPPORTED: bool = cfg!(target_os = "linux");

/// "`what` only runs on Linux" with what to do instead, for errors and notes.
pub fn linux_only_msg(what: &str) -> String {
    let os = std::env::consts::OS;
    let instead = if cfg!(target_os = "windows") {
        " On Windows Aurelia runs Windows games natively, without Proton/Wine."
    } else {
        ""
    };
    format!("{what} only runs on Linux, not on {os}.{instead}")
}

/// Fail early, before downloading or touching a prefix, when the Windows Steam
/// runtime can't run on this platform.
pub fn ensure_steam_runtime_supported() -> Result<()> {
    if STEAM_RUNTIME_SUPPORTED {
        return Ok(());
    }
    bail!("{}", steam_runtime_unsupported_msg())
}

/// Actionable message for when `steam_runtime_runner` is unset. `action` is the verb
/// phrase for the failing operation, e.g. "installing" / "repairing". Kept in one place
/// so every entry point tells the user the same concrete next steps: which runtimes are
/// available and exactly how to select one.
pub fn steam_runtime_runner_unset_msg(action: &str) -> String {
    format!(
        "no Steam Runtime Runner is configured — required for {action} the Windows Steam \
         runtime.\n  1. See installed runtimes: `aurelia proton list`\n  2. Select one: \
         `aurelia config steam-runtime-runner <NAME>` (e.g. GE-Proton9-20 or experimental)\n\
         A Proton runtime works — its bundled bare Wine is used automatically."
    )
}

/// Resolve the configured `steam_runtime_runner` *name* to the bare wine binary that
/// must host the Windows-Steam installer and the background Steam process.
///
/// This is deliberately NOT [`build_runner_command`]: that helper prefers a Proton
/// tree's `proton` script and yields `proton run <exe>`, which is wrong here on two
/// counts. `proton run` derives its own prefix from `STEAM_COMPAT_DATA_PATH` and
/// silently ignores the `WINEPREFIX` the caller set, and it expects to be inside the
/// Steam Linux Runtime container. When the configured runner is a Proton tree we use
/// the bare wine it bundles instead — the invariant `proton_bundled_bare_wine` and
/// `validate_steam_runtime_runner_path` already document.
pub fn resolve_steam_runtime_wine(runner_name: &str, library_root: &Path) -> Result<PathBuf> {
    if runner_name.trim().is_empty() {
        bail!("No Steam Runtime Runner selected in Global Settings");
    }

    // Quiet resolution: this function returns a proper Err on failure, so we don't want
    // resolve_runner's separate not-found warning firing here (it would double up with a
    // config-setter probe or the caller's own error).
    let resolved = resolve_runner_opt(runner_name, library_root).ok_or_else(|| {
        anyhow!(
            "Steam Runtime Runner `{runner_name}` could not be found on disk. \
             Install it (`aurelia proton install {runner_name}`), or point \
             `steam_runtime_runner` at a wine build. See `aurelia proton list`."
        )
    })?;

    // A Proton tree, or the `proton` script itself: take the wine it bundles rather
    // than the `proton run` wrapper.
    let is_proton_script =
        resolved.is_file() && resolved.file_name().and_then(|f| f.to_str()) == Some("proton");
    let is_proton_tree = resolved.is_dir() && classify_runner(&resolved) == RunnerKind::Proton;
    if is_proton_script || is_proton_tree {
        return proton_bundled_bare_wine(&resolved).ok_or_else(|| {
            anyhow!(
                "Steam Runtime Runner `{runner_name}` is a Proton tree with no bundled wine \
                 binary (looked for files/bin/wine64 and dist/bin/wine64 under {})",
                derive_runner_root(&resolved).display()
            )
        });
    }

    // Otherwise it must already be a bare wine binary or a bare wine tree.
    validate_steam_runtime_runner_path(&resolved)?;
    if resolved.is_file() {
        return Ok(resolved);
    }
    for rel in ["bin/wine64", "bin/wine"] {
        let candidate = resolved.join(rel);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    bail!(
        "Steam Runtime Runner `{runner_name}` has no wine binary under {}",
        resolved.display()
    )
}

/// Ensure the master Steam prefix has the runner's graphics support DLLs that a
/// **bare-wine** launch cannot otherwise resolve.
///
/// The `proton` wrapper script normally makes the runner's `vkd3d` and `dxvk`
/// PE libraries findable; a prefix driven by bare `wine64` (how Aurelia hosts the
/// Windows Steam runtime) misses them. The visible failure is Steam's CEF UI:
/// Chromium's GPU process does `LoadLibrary(dxgi.dll)` → Wine's `dxgi` needs
/// `wined3d` → `wined3d` needs `libvkd3d-1.dll`/`libvkd3d-shader-1.dll` → not
/// found → the load fails → Chromium CHECK-crashes (on real Windows `dxgi` can
/// never be absent) → `steamwebhelper` gives up after 3 GPU restarts → Steam
/// shows "There was a problem with your Steam installation. Please reinstall."
///
/// Copies into `system32` (and `syswow64` when present):
///   - the runner's DXVK `dxgi`/`d3d11`/… (self-contained, Vulkan-backed), and
///   - the runner's `libvkd3d-*` PE libs (so Wine's own `wined3d` chain also loads).
///
/// Handles both runner layouts (Proton 9: `lib64/…` + `lib/…`; Proton 10:
/// `lib/wine/dxvk/<arch>-windows`). Best-effort and idempotent: existing files are
/// overwritten so a stale copy from a different Wine can't wedge the prefix.
pub fn ensure_steam_runtime_prefix_libs(bare_wine: &Path, wine_prefix: &Path) {
    // <root>/files/bin/wine64 -> <root>/files
    let files_root = match bare_wine.parent().and_then(|p| p.parent()) {
        Some(root) => root.to_path_buf(),
        None => return,
    };

    // (candidate source dirs, destination) per architecture. The first candidate
    // that exists and contains DLLs wins for each group.
    let sys32 = wine_prefix.join("drive_c/windows/system32");
    let syswow = wine_prefix.join("drive_c/windows/syswow64");
    let groups: [(&[&str], &Path); 4] = [
        // 64-bit DXVK → system32
        (&["lib64/wine/dxvk", "lib/wine/dxvk/x86_64-windows"], &sys32),
        // 64-bit vkd3d support libs → system32
        (&["lib64/vkd3d", "lib/vkd3d/x86_64-windows"], &sys32),
        // 32-bit DXVK → syswow64
        (&["lib/wine/dxvk", "lib/wine/dxvk/i386-windows"], &syswow),
        // 32-bit vkd3d support libs → syswow64
        (&["lib/vkd3d", "lib/vkd3d/i386-windows"], &syswow),
    ];

    for (candidates, dest) in groups {
        if !dest.is_dir() {
            continue;
        }
        for rel in candidates {
            let src = files_root.join(rel);
            let Ok(entries) = std::fs::read_dir(&src) else {
                continue;
            };
            let mut copied = 0usize;
            for entry in entries.flatten() {
                let path = entry.path();
                let is_dll = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("dll"));
                if !is_dll {
                    continue;
                }
                if let Some(name) = path.file_name() {
                    match std::fs::copy(&path, dest.join(name)) {
                        Ok(_) => copied += 1,
                        Err(e) => tracing::warn!(
                            "could not copy {} into {}: {e}",
                            path.display(),
                            dest.display()
                        ),
                    }
                }
            }
            if copied > 0 {
                tracing::info!(
                    "Steam-runtime prefix libs: copied {copied} DLL(s) from {} to {}",
                    src.display(),
                    dest.display()
                );
                break; // first matching layout wins for this group
            }
        }
    }
}

pub fn derive_runner_root(binary_path: &Path) -> PathBuf {
    let parent = if binary_path.is_file() {
        binary_path.parent().unwrap_or(binary_path)
    } else {
        binary_path
    };
    // If it's in a 'bin' directory (like wine-tkg), the root is one level up
    if parent.file_name().is_some_and(|n| n == "bin") {
        return parent.parent().unwrap_or(parent).to_path_buf();
    }

    // Otherwise (like proton script), the root is the parent directory
    parent.to_path_buf()
}

#[cfg(test)]
mod tests;
