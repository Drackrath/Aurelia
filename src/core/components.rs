//! DXVK/VKD3D/NVAPI component detection.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::core::runner::derive_runner_root;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunnerComponents {
    pub dxvk: Option<ComponentInfo>,
    pub vkd3d_proton: Option<ComponentInfo>,
    pub vkd3d: Option<ComponentInfo>,
    pub nvapi: Option<ComponentInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentInfo {
    pub version: String,
    pub source: ComponentSource,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ComponentSource {
    BundledWithRunner,
    InstalledInPrefix,
    SystemWide,
}

impl std::fmt::Display for ComponentSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BundledWithRunner => write!(f, "bundled"),
            Self::InstalledInPrefix => write!(f, "in prefix"),
            Self::SystemWide => write!(f, "system"),
        }
    }
}

pub fn detect_runner_components(runner_path: &Path, wineprefix: Option<&Path>) -> RunnerComponents {
    let root = derive_runner_root(runner_path);

    RunnerComponents {
        dxvk: detect_dxvk(&root, wineprefix),
        vkd3d_proton: detect_vkd3d_proton(&root, wineprefix),
        vkd3d: detect_vkd3d(&root, wineprefix),
        nvapi: detect_nvapi(&root, wineprefix),
    }
}

// ── DXVK ────────────────────────────────────────────────────────────────────

fn detect_dxvk(root: &Path, prefix: Option<&Path>) -> Option<ComponentInfo> {
    // 1. Bundled inside runner (Modern Wine-TKG layout)
    if let Some(info) = detect_bundled_modern(
        root,
        &crate::compat::proton::wine_component_dirs("dxvk"),
        |_| {
            &[
                "d3d11.dll",
                "dxgi.dll",
                "d3d9.dll",
                "d3d8.dll",
                "d3d10core.dll",
            ]
        },
    ) {
        return Some(info);
    }

    // Legacy/Proton fallback
    if let Some(info) = check_bundled_legacy(root, "dxvk", "d3d11.dll") {
        return Some(info);
    }

    // 2. Installed into WINEPREFIX (winetricks / manual)
    if let Some(pfx) = prefix {
        let prefix_dlls = [
            "drive_c/windows/system32/d3d11.dll",
            "drive_c/windows/syswow64/d3d11.dll",
        ];
        if let Some(info) = check_prefix(pfx, &prefix_dlls) {
            return Some(info);
        }
    }

    // 3. System-wide (package manager install)
    let system_paths = [
        "/usr/share/dxvk/x64/d3d11.dll",
        "/usr/lib/dxvk/d3d11.dll",
        "/usr/lib/x86_64-linux-gnu/dxvk/d3d11.dll",
        "/usr/local/share/dxvk/x64/d3d11.dll",
    ];
    check_system(&system_paths)
}

// ── VKD3D-Proton ─────────────────────────────────────────────────────────────

fn detect_vkd3d_proton(root: &Path, prefix: Option<&Path>) -> Option<ComponentInfo> {
    // 1. Modern Wine-TKG layout
    if let Some(info) = detect_bundled_modern(
        root,
        &crate::compat::proton::wine_component_dirs("vkd3d-proton"),
        |_| &["d3d12.dll", "d3d12core.dll"],
    ) {
        return Some(info);
    }

    // Legacy/Proton fallback
    if let Some(info) = check_bundled_legacy(root, "vkd3d-proton", "d3d12.dll") {
        return Some(info);
    }

    // VKD3D-Proton replaces d3d12.dll — check prefix for it
    if let Some(pfx) = prefix
        && let Some(info) = check_prefix_d3d12(pfx, true)
    {
        return Some(info);
    }

    let system_paths = [
        "/usr/share/vkd3d-proton/x64/d3d12.dll",
        "/usr/lib/vkd3d-proton/d3d12.dll",
        "/usr/local/share/vkd3d-proton/x64/d3d12.dll",
    ];
    check_system(&system_paths)
}

// ── VKD3D (upstream) ─────────────────────────────────────────────────────────

fn detect_nvapi(root: &Path, prefix: Option<&Path>) -> Option<ComponentInfo> {
    // 1. Bundled inside runner (Modern Wine-TKG layout)
    if let Some(info) = detect_bundled_modern(
        root,
        &crate::compat::proton::wine_component_dirs("nvapi"),
        |arch| {
            if arch == "x86_64-windows" {
                &["nvapi64.dll"]
            } else {
                &["nvapi.dll"]
            }
        },
    ) {
        return Some(info);
    }

    // 2. Installed into WINEPREFIX
    if let Some(pfx) = prefix {
        let prefix_dlls = [
            "drive_c/windows/system32/nvapi64.dll",
            "drive_c/windows/syswow64/nvapi.dll",
        ];
        if let Some(info) = check_prefix(pfx, &prefix_dlls) {
            return Some(info);
        }
    }

    None
}

fn detect_vkd3d(root: &Path, prefix: Option<&Path>) -> Option<ComponentInfo> {
    // 1. Modern Wine-TKG layout
    if let Some(info) = detect_bundled_modern(
        root,
        &crate::compat::proton::wine_component_dirs("vkd3d"),
        |_| &["libvkd3d-1.dll", "libvkd3d-shader-1.dll"],
    ) {
        return Some(info);
    }

    // Legacy/Proton fallback (upstream libvkd3d-1.dll).
    if let Some(info) = check_bundled_legacy(root, "vkd3d", "libvkd3d-1.dll") {
        return Some(info);
    }

    if let Some(pfx) = prefix
        && let Some(info) = check_prefix_d3d12(pfx, false)
    {
        return Some(info);
    }

    let system_paths = [
        "/usr/lib/x86_64-linux-gnu/libvkd3d.so.1",
        "/usr/lib64/libvkd3d.so.1",
        "/usr/local/lib/libvkd3d.so.1",
    ];
    check_system(&system_paths)
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Detect a component bundled inside a runner using the modern Wine-TKG layout:
/// `<root>/<subdir>/<arch>/<dlls>` with a `version` file in the arch or component
/// folder. `required_for_arch` yields the DLLs that must all be present for a given
/// arch (NVAPI ships arch-specific names, the rest share one list).
fn detect_bundled_modern<S: AsRef<str>>(
    root: &Path,
    subdirs: &[S],
    required_for_arch: impl Fn(&str) -> &'static [&'static str],
) -> Option<ComponentInfo> {
    for subdir in subdirs {
        let comp_path = root.join(subdir.as_ref());
        if !comp_path.is_dir() {
            continue;
        }
        // Only the Windows PE arch dirs hold the DLLs we match here; the `-unix`
        // WOW64 bridge dirs from ARCH_SUBDIRS are irrelevant to this check.
        for arch in crate::compat::proton::ARCH_SUBDIRS
            .iter()
            .filter(|a| a.ends_with("-windows"))
        {
            let arch = *arch;
            let arch_path = comp_path.join(arch);
            let required = required_for_arch(arch);
            if required.iter().all(|dll| arch_path.join(dll).exists()) {
                let version = ["version", "../version"] // check in arch or component folder
                    .iter()
                    .filter_map(|v| std::fs::read_to_string(arch_path.join(v)).ok())
                    .map(|s| parse_short_version(&s))
                    .find(|s| s != "unknown")
                    .unwrap_or_else(|| "found".to_string());
                return Some(ComponentInfo {
                    version,
                    source: ComponentSource::BundledWithRunner,
                    path: Some(arch_path),
                });
            }
        }
    }
    None
}

/// Legacy bundled-layout probe per component.
fn check_bundled_legacy(root: &Path, component: &str, dll: &str) -> Option<ComponentInfo> {
    let dll_candidates: Vec<String> = [
        "files/lib64",
        "files/lib",
        "dist/lib64",
        "dist/lib",
        "lib64",
        "lib",
    ]
    .iter()
    .map(|base| format!("{base}/wine/{component}/{dll}"))
    .collect();
    let version_files: Vec<String> = ["files/share", "dist/share", "share"]
        .iter()
        .map(|base| format!("{base}/{component}/version"))
        .collect();
    let dll_refs: Vec<&str> = dll_candidates.iter().map(String::as_str).collect();
    let version_refs: Vec<&str> = version_files.iter().map(String::as_str).collect();
    check_bundled(root, &dll_refs, &version_refs)
}

/// Attribute prefix `d3d12.dll` by marker.
fn check_prefix_d3d12(pfx: &Path, want_proton: bool) -> Option<ComponentInfo> {
    for rel in [
        "drive_c/windows/system32/d3d12.dll",
        "drive_c/windows/syswow64/d3d12.dll",
    ] {
        let p = pfx.join(rel);
        if p.exists() && dll_contains_string(&p, "vkd3d-proton") == want_proton {
            let version = extract_version_from_dll(&p).unwrap_or_else(|| "unknown".to_string());
            return Some(ComponentInfo {
                version,
                source: ComponentSource::InstalledInPrefix,
                path: Some(p),
            });
        }
    }
    None
}

fn check_bundled(
    root: &Path,
    dll_candidates: &[&str],
    version_files: &[&str],
) -> Option<ComponentInfo> {
    let found_dll = dll_candidates.iter().find(|rel| root.join(rel).exists())?;
    tracing::debug!(
        "Found bundled component DLL at: {}",
        root.join(found_dll).display()
    );

    let version = version_files
        .iter()
        .filter_map(|rel| {
            let p = root.join(rel);
            if p.exists() {
                tracing::debug!("Found version file: {}", p.display());
                std::fs::read_to_string(p).ok()
            } else {
                None
            }
        })
        .map(|s| parse_short_version(&s))
        .find(|s| s != "unknown")
        .or_else(|| {
            dll_candidates
                .iter()
                .map(|rel| root.join(rel))
                .find(|p| p.exists())
                .and_then(|p| extract_version_from_dll(&p))
        })
        .unwrap_or_else(|| "unknown".to_string());

    Some(ComponentInfo {
        version,
        source: ComponentSource::BundledWithRunner,
        path: root.join(found_dll).parent().map(|p| p.to_path_buf()),
    })
}

fn check_prefix(prefix: &Path, dll_candidates: &[&str]) -> Option<ComponentInfo> {
    for rel in dll_candidates {
        let p = prefix.join(rel);
        if p.exists() {
            // Exclude Wine's own built-in wined3d stubs (very small, < 50KB)
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            if size < 51_200 {
                continue;
            }

            let version = extract_version_from_dll(&p).unwrap_or_else(|| "unknown".to_string());
            return Some(ComponentInfo {
                version,
                source: ComponentSource::InstalledInPrefix,
                path: Some(p),
            });
        }
    }
    None
}

fn check_system(paths: &[&str]) -> Option<ComponentInfo> {
    for path in paths {
        let p = Path::new(path);
        if p.exists() {
            let version = extract_version_from_dll(p)
                .or_else(|| read_adjacent_version_file(p))
                .unwrap_or_else(|| "unknown".to_string());
            return Some(ComponentInfo {
                version,
                source: ComponentSource::SystemWide,
                path: Some(p.to_path_buf()),
            });
        }
    }
    None
}

fn read_adjacent_version_file(dll: &Path) -> Option<String> {
    let parent = dll.parent()?;
    let version_file = parent.join("version");
    std::fs::read_to_string(version_file)
        .ok()
        .map(|s| parse_short_version(&s))
        .filter(|s| !s.is_empty())
}

pub fn parse_short_version(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return "unknown".to_string();
    }

    // Try to find content inside parentheses first (Wine-TKG style)
    let v = if let (Some(start), Some(end)) = (s.find('('), s.rfind(')')) {
        if start < end { &s[start + 1..end] } else { s }
    } else {
        // If no parentheses, it might be a simple version string
        // or a Wine-TKG style without () but with multiple space-separated parts.
        if s.contains(' ') {
            s.split_whitespace().last().unwrap_or(s)
        } else {
            s
        }
    };

    let mut v = v.trim();

    // Strip component name prefixes (like 'dxvk-', 'vkd3d-proton-', 'vkd3d-')
    for prefix in &["vkd3d-proton-", "vkd3d-", "dxvk-"] {
        if let Some(stripped) = v.strip_prefix(prefix) {
            v = stripped;
            break;
        }
    }

    // Strip leading 'v' if followed by a digit
    if v.starts_with('v') && v.len() > 1 && v.as_bytes()[1].is_ascii_digit() {
        v = &v[1..];
    }

    // Strip trailing git hash suffix: -g[0-9a-f]{7,10}
    if let Some(hyphen_idx) = v.rfind("-g") {
        let suffix = &v[hyphen_idx + 2..];
        if suffix.len() >= 7 && suffix.len() <= 10 && suffix.chars().all(|c| c.is_ascii_hexdigit())
        {
            return v[..hyphen_idx].to_string();
        }
    }

    v.to_string()
}

fn dll_contains_string(path: &Path, needle: &str) -> bool {
    let needle_lower = needle.to_ascii_lowercase();
    std::fs::read(path)
        .map(|bytes| {
            bytes.windows(needle.len()).any(|w| {
                w.iter()
                    .zip(needle_lower.bytes())
                    .all(|(b, n)| b.to_ascii_lowercase() == n)
            })
        })
        .unwrap_or(false)
}

fn extract_version_from_dll(dll_path: &Path) -> Option<String> {
    let data = std::fs::read(dll_path).ok()?;

    // Collect all printable ASCII runs of length >= 4
    let mut runs: Vec<String> = Vec::new();
    let mut current = Vec::new();
    for &byte in &data {
        if (0x20..0x7f).contains(&byte) {
            current.push(byte as char);
        } else {
            if current.len() >= 4 {
                runs.push(current.iter().collect());
            }
            current.clear();
        }
    }
    if current.len() >= 4 {
        runs.push(current.iter().collect());
    }

    // Match semver-like patterns: optional 'v', digits, dots, optional suffix
    // e.g. "2.3.1", "v1.10.3", "2.4-dirty", "v2.0.0-alpha.1+git"
    static SEMVER_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^v?(\d{1,3})\.(\d{1,3})(\.\d{1,3})?([-.][a-zA-Z0-9._-]+)?$").unwrap()
    });

    // Prefer strings that look like "vX.Y.Z" over bare "X.Y"
    let mut candidates: Vec<String> = runs
        .into_iter()
        .filter(|s| SEMVER_RE.is_match(s))
        .filter(|s| {
            // Exclude obviously non-version strings (all zeros, single digit etc.)
            let parts: Vec<&str> = s.trim_start_matches('v').splitn(2, '.').collect();
            parts.len() >= 2 && parts[0].parse::<u32>().unwrap_or(100) <= 99
        })
        .collect();

    // Sort: longer (more specific) versions first
    candidates.sort_by_key(|c| std::cmp::Reverse(c.len()));
    candidates.into_iter().next()
}
