use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
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

    bail!("Failed to resolve a valid runner binary from {}", runner_path.display())
}

pub fn resolve_runner(name: &str, library_root: &Path) -> PathBuf {
    let name_path = Path::new(name);
    if name_path.is_absolute() || name_path.exists() {
        return name_path.to_path_buf();
    }

    // 1. Steam Library (steamapps/common)
    let steam_path = library_root.join("steamapps/common").join(name);
    if steam_path.exists() {
        return steam_path;
    }

    // 2. compatibilitytools.d (Steam Custom)
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let compat_path = PathBuf::from(&home).join(".local/share/Steam/compatibilitytools.d").join(name);
    if compat_path.exists() {
        return compat_path;
    }

    // 3. Lutris Runners
    let lutris_path = PathBuf::from(&home).join(".local/share/lutris/runners/wine").join(name);
    if lutris_path.exists() {
        return lutris_path;
    }

    // 4. Fallback to name as provided
    name_path.to_path_buf()
}

pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
    std::fs::create_dir_all(&dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

pub fn setup_fake_steam_trap(config_dir: &Path) -> Result<PathBuf> {
    let trap_dir = config_dir.join("fake_env");
    std::fs::create_dir_all(&trap_dir)?;

    let dummy_script = "#!/bin/sh\nexit 0\n";

    let steam_path = trap_dir.join("steam");
    let steam_sh_path = trap_dir.join("steam.sh");

    if !steam_path.exists() {
        std::fs::write(&steam_path, dummy_script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&steam_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&steam_path, perms)?;
        }
    }

    if !steam_sh_path.exists() {
        std::fs::write(&steam_sh_path, dummy_script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&steam_sh_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&steam_sh_path, perms)?;
        }
    }

    Ok(trap_dir)
}

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

pub fn derive_runner_root(binary_path: &Path) -> PathBuf {
    let parent = if binary_path.is_file() {
        binary_path.parent().unwrap_or(binary_path)
    } else {
        binary_path
    };
    // If it's in a 'bin' directory (like wine-tkg), the root is one level up
    if parent.file_name().map(|n| n == "bin").unwrap_or(false) {
        return parent.parent().unwrap_or(parent).to_path_buf();
    }

    // Otherwise (like proton script), the root is the parent directory
    parent.to_path_buf()
}

pub fn detect_runner_components(
    runner_path: &Path,
    wineprefix: Option<&Path>,
) -> RunnerComponents {
    let root = derive_runner_root(runner_path);

    let (dxvk, vkd3d_proton, vkd3d, nvapi) = (
        detect_dxvk(&root, wineprefix),
        detect_vkd3d_proton(&root, wineprefix),
        detect_vkd3d(&root, wineprefix),
        detect_nvapi(&root, wineprefix),
    );


    RunnerComponents {
        dxvk,
        vkd3d_proton,
        vkd3d,
        nvapi,
    }
}

/// Detects NVIDIA Optimus / hybrid graphics and returns the env vars needed
/// to force the discrete NVIDIA GPU. Returns empty map on non-hybrid systems.
pub fn detect_prime_env() -> std::collections::HashMap<String, String> {
    let mut vars = std::collections::HashMap::new();

    let has_nvidia_dev = std::path::Path::new("/dev/nvidia0").exists()
        || std::path::Path::new("/proc/driver/nvidia").exists();
    // Check for a second DRM device (the integrated one)
    let has_igpu = std::path::Path::new("/dev/dri/card1").exists();

    if has_nvidia_dev && has_igpu {
        // Optimus: force discrete NVIDIA for both Vulkan and OpenGL
        vars.insert("__NV_PRIME_RENDER_OFFLOAD".to_string(), "1".to_string());
        vars.insert(
            "__NV_PRIME_RENDER_OFFLOAD_PROVIDER".to_string(),
            "NVIDIA-G0".to_string(),
        );
        vars.insert(
            "__VK_LAYER_NV_optimus".to_string(),
            "NVIDIA_only".to_string(),
        );
        vars.insert("__GLX_VENDOR_LIBRARY_NAME".to_string(), "nvidia".to_string());

        // Also hint VKD3D-Proton via its own knob
        if let Ok(val) = std::env::var("VKD3D_FEATURE_FLAGS") {
            vars.insert("VKD3D_FEATURE_FLAGS".to_string(), val);
        }
    }

    vars
}

// ── DXVK ────────────────────────────────────────────────────────────────────

fn detect_dxvk(root: &Path, prefix: Option<&Path>) -> Option<ComponentInfo> {
    // 1. Bundled inside runner (Modern Wine-TKG layout)
    let comp_subdirs = ["lib/wine/dxvk", "files/lib/wine/dxvk", "dist/lib/wine/dxvk"];
    let required = ["d3d11.dll", "dxgi.dll", "d3d9.dll", "d3d8.dll", "d3d10core.dll"];

    for subdir in comp_subdirs {
        let comp_path = root.join(subdir);
        if comp_path.is_dir() {
            // Check arch subfolders
            for arch in ["x86_64-windows", "i386-windows"] {
                let arch_path = comp_path.join(arch);
                if required.iter().all(|dll| arch_path.join(dll).exists()) {
                    let version = ["version", "../version"] // check in arch or component folder
                        .iter()
                        .filter_map(|v| {
                            let p = arch_path.join(v);
                            std::fs::read_to_string(p).ok()
                        })
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
    }

    // Legacy/Proton fallback
    let bundled_dlls = [
        "files/lib64/wine/dxvk/d3d11.dll",
        "files/lib/wine/dxvk/d3d11.dll",
        "dist/lib64/wine/dxvk/d3d11.dll",
        "dist/lib/wine/dxvk/d3d11.dll",
        "lib64/wine/dxvk/d3d11.dll",
        "lib/wine/dxvk/d3d11.dll",
    ];
    if let Some(info) = check_bundled(
        root,
        &bundled_dlls,
        &[
            "files/share/dxvk/version",
            "dist/share/dxvk/version",
            "share/dxvk/version",
        ],
    ) {
        return Some(info);
    }

    // 2. Installed into WINEPREFIX (winetricks / manual)
    if let Some(pfx) = prefix {
        let prefix_dlls = [
            "drive_c/windows/system32/d3d11.dll",
            "drive_c/windows/syswow64/d3d11.dll",
        ];
        if let Some(info) = check_prefix(pfx, &prefix_dlls, "DXVK") {
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
    let comp_subdirs = ["lib/wine/vkd3d-proton", "files/lib/wine/vkd3d-proton", "dist/lib/wine/vkd3d-proton"];
    let required = ["d3d12.dll", "d3d12core.dll"];

    for subdir in comp_subdirs {
        let comp_path = root.join(subdir);
        if comp_path.is_dir() {
            for arch in ["x86_64-windows", "i386-windows"] {
                let arch_path = comp_path.join(arch);
                if required.iter().all(|dll| arch_path.join(dll).exists()) {
                    let version = ["version", "../version"]
                        .iter()
                        .filter_map(|v| {
                            let p = arch_path.join(v);
                            std::fs::read_to_string(p).ok()
                        })
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
    }

    // Legacy/Proton fallback
    let bundled_dlls = [
        "files/lib64/wine/vkd3d-proton/d3d12.dll",
        "files/lib/wine/vkd3d-proton/d3d12.dll",
        "dist/lib64/wine/vkd3d-proton/d3d12.dll",
        "dist/lib/wine/vkd3d-proton/d3d12.dll",
        "lib64/wine/vkd3d-proton/d3d12.dll",
        "lib/wine/vkd3d-proton/d3d12.dll",
    ];
    if let Some(info) = check_bundled(
        root,
        &bundled_dlls,
        &[
            "files/share/vkd3d-proton/version",
            "dist/share/vkd3d-proton/version",
            "share/vkd3d-proton/version",
        ],
    ) {
        return Some(info);
    }

    // VKD3D-Proton replaces d3d12.dll — check prefix for it
    if let Some(pfx) = prefix {
        let prefix_dlls = [
            "drive_c/windows/system32/d3d12.dll",
            "drive_c/windows/syswow64/d3d12.dll",
        ];
        for rel in prefix_dlls {
            let p = pfx.join(rel);
            if p.exists() {
                if dll_contains_string(&p, "vkd3d-proton") {
                    let version = extract_version_from_dll(&p).unwrap_or_else(|| "unknown".to_string());
                    return Some(ComponentInfo {
                        version,
                        source: ComponentSource::InstalledInPrefix,
                        path: Some(p),
                    });
                }
            }
        }
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
    let comp_subdirs = ["lib/wine/nvapi", "files/lib/wine/nvapi", "dist/lib/wine/nvapi"];

    for subdir in comp_subdirs {
        let comp_path = root.join(subdir);
        if comp_path.is_dir() {
            // Check arch subfolders
            for arch in ["x86_64-windows", "i386-windows"] {
                let arch_path = comp_path.join(arch);
                let dlls = if arch == "x86_64-windows" {
                    vec!["nvapi64.dll"]
                } else {
                    vec!["nvapi.dll"]
                };

                if dlls.iter().all(|dll| arch_path.join(dll).exists()) {
                    let version = ["version", "../version"]
                        .iter()
                        .filter_map(|v| {
                            let p = arch_path.join(v);
                            std::fs::read_to_string(p).ok()
                        })
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
    }

    // 2. Installed into WINEPREFIX
    if let Some(pfx) = prefix {
        let prefix_dlls = [
            "drive_c/windows/system32/nvapi64.dll",
            "drive_c/windows/syswow64/nvapi.dll",
        ];
        if let Some(info) = check_prefix(pfx, &prefix_dlls, "NVAPI") {
            return Some(info);
        }
    }

    None
}

fn detect_vkd3d(root: &Path, prefix: Option<&Path>) -> Option<ComponentInfo> {
    // 1. Modern Wine-TKG layout
    let comp_subdirs = ["lib/wine/vkd3d", "files/lib/wine/vkd3d", "dist/lib/wine/vkd3d"];
    let required = ["libvkd3d-1.dll", "libvkd3d-shader-1.dll"];

    for subdir in comp_subdirs {
        let comp_path = root.join(subdir);
        if comp_path.is_dir() {
            for arch in ["x86_64-windows", "i386-windows"] {
                let arch_path = comp_path.join(arch);
                if required.iter().all(|dll| arch_path.join(dll).exists()) {
                    let version = ["version", "../version"]
                        .iter()
                        .filter_map(|v| {
                            let p = arch_path.join(v);
                            std::fs::read_to_string(p).ok()
                        })
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
    }

    // Legacy/Proton fallback
    // Upstream Wine VKD3D uses libvkd3d.dll/libvkd3d-1.dll and libvkd3d-shader.dll
    let bundled_dlls = [
        "files/lib64/wine/vkd3d/libvkd3d-1.dll",
        "files/lib/wine/vkd3d/libvkd3d-1.dll",
        "dist/lib64/wine/vkd3d/libvkd3d-1.dll",
        "dist/lib/wine/vkd3d/libvkd3d-1.dll",
        "lib64/wine/vkd3d/libvkd3d-1.dll",
        "lib/wine/vkd3d/libvkd3d-1.dll",
    ];
    if let Some(info) = check_bundled(
        root,
        &bundled_dlls,
        &[
            "files/share/vkd3d/version",
            "dist/share/vkd3d/version",
            "share/vkd3d/version",
        ],
    ) {
        return Some(info);
    }

    if let Some(pfx) = prefix {
        let prefix_dlls = [
            "drive_c/windows/system32/d3d12.dll",
            "drive_c/windows/syswow64/d3d12.dll",
        ];
        for rel in prefix_dlls {
            let p = pfx.join(rel);
            if p.exists() && !dll_contains_string(&p, "vkd3d-proton") {
                let version = extract_version_from_dll(&p).unwrap_or_else(|| "unknown".to_string());
                return Some(ComponentInfo {
                    version,
                    source: ComponentSource::InstalledInPrefix,
                    path: Some(p),
                });
            }
        }
    }

    let system_paths = [
        "/usr/lib/x86_64-linux-gnu/libvkd3d.so.1",
        "/usr/lib64/libvkd3d.so.1",
        "/usr/local/lib/libvkd3d.so.1",
    ];
    check_system(&system_paths)
}

// ── Shared helpers ────────────────────────────────────────────────────────────

fn check_bundled(root: &Path, dll_candidates: &[&str], version_files: &[&str]) -> Option<ComponentInfo> {
    let found_dll = dll_candidates.iter().find(|rel| root.join(rel).exists());
    if let Some(rel) = found_dll {
        tracing::debug!("Found bundled component DLL at: {}", root.join(rel).display());
    } else {
        return None;
    }

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
        path: root.join(found_dll.unwrap()).parent().map(|p| p.to_path_buf()),
    })
}

fn check_prefix(prefix: &Path, dll_candidates: &[&str], _name: &str) -> Option<ComponentInfo> {
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
        if start < end {
            &s[start + 1..end]
        } else {
            s
        }
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
        if v.starts_with(prefix) {
            v = &v[prefix.len()..];
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
        if !suffix.is_empty()
            && suffix.len() >= 7
            && suffix.len() <= 10
            && suffix.chars().all(|c| c.is_ascii_hexdigit())
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
        if byte >= 0x20 && byte < 0x7f {
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
    let semver_re = regex::Regex::new(r"^v?(\d{1,3})\.(\d{1,3})(\.\d{1,3})?([-.][a-zA-Z0-9._-]+)?$").ok()?;

    // Prefer strings that look like "vX.Y.Z" over bare "X.Y"
    let mut candidates: Vec<String> = runs
        .into_iter()
        .filter(|s| semver_re.is_match(s))
        .filter(|s| {
            // Exclude obviously non-version strings (all zeros, single digit etc.)
            let parts: Vec<&str> = s.trim_start_matches('v').splitn(2, '.').collect();
            parts.len() >= 2 && parts[0].parse::<u32>().unwrap_or(100) <= 99
        })
        .collect();

    // Sort: longer (more specific) versions first
    candidates.sort_by(|a, b| b.len().cmp(&a.len()));
    candidates.into_iter().next()
}

#[derive(Debug, Clone, PartialEq)]
pub enum GraphicsLayer {
    Dxvk,
    Vkd3dProton,
    Vkd3d,
}

/// Returns the WINEDLLOVERRIDES string needed to activate installed layers.
pub fn build_dll_overrides(
    dxvk_active: bool,
    vkd3d_proton_active: bool,
    vkd3d_active: bool,
    no_overlay: bool,
    force_builtin_d3d: bool, // NEW — for WineD3D policy
    game_dir: Option<&std::path::Path>, // check for game-local DLLs
    strict_dxvk: bool,
) -> String {
    let mut overrides: Vec<String> = vec![
        "vstdlib_s=n".into(),
        "tier0_s=n".into(),
        "steamclient=n".into(),
        "steamclient64=n".into(),
        "steam_api=n".into(),
        "steam_api64=n".into(),
        "lsteamclient=".into(),
    ];

    if no_overlay {
        overrides.push("GameOverlayRenderer=n".into());
        overrides.push("GameOverlayRenderer64=n".into());
    }

    if force_builtin_d3d {
        // Explicitly force Wine's own builtins for all D3D DLLs.
        // This overrides any native DLL sitting in the prefix's system32
        // from a previous DXVK/VKD3D install.
        for dll in &[
            "d3d8",
            "d3d9",
            "d3d10core",
            "d3d11",
            "dxgi",
            "d3d12",
            "d3d12core",
        ] {
            overrides.push(format!("{dll}=b"));
        }
        return overrides.join(";");
    }

    if dxvk_active {
        // If the game ships its own d3d DLLs, don't fight them — just
        // ensure native wins without specifying which native.
        // Wine searches exe-dir before system32, so "n,b" is fine UNLESS
        // a foreign dll landed in system32. We skip the override entirely
        // for DLLs the game already provides locally.
        let game_has = |dll: &str| -> bool { game_dir.map(|d| d.join(dll).exists()).unwrap_or(false) };

        for dll in &[
            "d3d8.dll",
            "d3d9.dll",
            "d3d10core.dll",
            "d3d11.dll",
            "dxgi.dll",
        ] {
            let stem = dll.trim_end_matches(".dll");
            let mode = if strict_dxvk { "n" } else { "n,b" };

            if strict_dxvk || !game_has(dll) {
                overrides.push(format!("{stem}={mode}"));
            }
            // If the game ships it locally and we are not in strict mode,
            // leave Wine's default search order alone — exe-dir native wins automatically.
        }
    }

    if vkd3d_proton_active || vkd3d_active {
        overrides.push("d3d12=n,b".into());
        overrides.push("d3d12core=n,b".into());
        if vkd3d_active {
            overrides.push("libvkd3d-1=n,b".into());
            overrides.push("libvkd3d-shader-1=n,b".into());
        }
    }

    overrides.join(";")
}

#[derive(Debug, Clone)]
pub struct MasterSteamConfig {
    pub root_dir: PathBuf,      // e.g. ~/.config/Aurelia/master_steam_prefix
    pub wine_prefix: PathBuf,   // e.g. root_dir or root_dir/pfx
    pub layout_kind: String,    // "root" or "pfx"
    pub steam_exe: Option<PathBuf>,
}

pub fn get_master_steam_config() -> MasterSteamConfig {
    let root_dir = crate::config::config_dir()
        .unwrap_or_default()
        .join("master_steam_prefix");

    // Layout detection: prefer /pfx if it exists, otherwise check root for drive_c
    let (wine_prefix, layout_kind) = if root_dir.join("pfx/drive_c").exists() {
        (root_dir.join("pfx"), "pfx".to_string())
    } else if root_dir.join("drive_c").exists() {
        (root_dir.clone(), "root".to_string())
    } else {
        // Default for new installs
        (root_dir.join("pfx"), "pfx".to_string())
    };

    let steam_exe = find_steam_exe_in_prefix(&wine_prefix);

    MasterSteamConfig {
        root_dir,
        wine_prefix,
        layout_kind,
        steam_exe,
    }
}

pub fn find_steam_exe_in_prefix(prefix: &Path) -> Option<PathBuf> {
    let candidates = [
        "drive_c/Program Files (x86)/Steam/steam.exe",
        "drive_c/Program Files/Steam/steam.exe",
    ];

    for rel_path in candidates {
        let full_path = prefix.join(rel_path);
        if full_path.exists() {
            return Some(full_path);
        }
    }

    None
}

/// Detects the actual WINEPREFIX layout for the master Steam install.
/// Handles both master_steam_prefix/pfx/drive_c and master_steam_prefix/drive_c layouts.
pub fn resolve_master_wineprefix() -> PathBuf {
    get_master_steam_config().wine_prefix
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedGpu {
    pub name: String,
    pub pci_id: Option<String>,
    pub is_discrete: bool,
}

pub fn list_available_gpus() -> Vec<DetectedGpu> {
    let mut gpus = Vec::new();

    // Try scanning /sys/class/drm/card* to find GPUs
    // This is more reliable than just checking /dev/dri/
    let drm_path = Path::new("/sys/class/drm");
    if let Ok(entries) = std::fs::read_dir(drm_path) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("card") && !name.contains('-') {
                let card_path = entry.path();

                // Read vendor and device IDs if available
                let device_path = card_path.join("device");
                let vendor = std::fs::read_to_string(device_path.join("vendor"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                let device = std::fs::read_to_string(device_path.join("device"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();

                let pci_id = if !vendor.is_empty() && !device.is_empty() {
                    Some(format!("{}:{}", vendor.replace("0x", ""), device.replace("0x", "")))
                } else {
                    None
                };

                // Heuristic for discrete vs integrated
                // This is a bit simplified, but often works on Linux
                let is_discrete = pci_id.as_ref().map(|id| {
                    // NVIDIA, AMD (discrete), etc.
                    id.starts_with("10de") || id.starts_with("1002")
                }).unwrap_or(false);

                let gpu_name = match pci_id.as_deref() {
                    Some(id) if id.starts_with("10de") => format!("NVIDIA GPU ({})", name),
                    Some(id) if id.starts_with("1002") => format!("AMD GPU ({})", name),
                    Some(id) if id.starts_with("8086") => format!("Intel GPU ({})", name),
                    _ => format!("Unknown GPU ({})", name),
                };

                gpus.push(DetectedGpu {
                    name: gpu_name,
                    pci_id,
                    is_discrete,
                });
            }
        }
    }

    // Fallback if /sys scan failed but we have NVIDIA tools or similar
    if gpus.is_empty() {
        if Path::new("/dev/nvidia0").exists() {
             gpus.push(DetectedGpu {
                 name: "NVIDIA Discrete GPU".to_string(),
                 pci_id: Some("10de:unknown".to_string()),
                 is_discrete: true,
             });
        }
    }

    gpus.sort_by(|a, b| b.is_discrete.cmp(&a.is_discrete));
    gpus
}

pub fn detect_exe_architecture(exe_path: &Path) -> crate::models::ExecutableArchitecture {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = match std::fs::File::open(exe_path) {
        Ok(f) => f,
        Err(_) => return crate::models::ExecutableArchitecture::Unknown,
    };

    let mut mz_header = [0u8; 2];
    if file.read_exact(&mut mz_header).is_err() || &mz_header != b"MZ" {
        return crate::models::ExecutableArchitecture::Unknown;
    }

    // Offset 0x3C contains the offset to the PE header
    if file.seek(SeekFrom::Start(0x3C)).is_err() {
        return crate::models::ExecutableArchitecture::Unknown;
    }

    let mut pe_offset_buf = [0u8; 4];
    if file.read_exact(&mut pe_offset_buf).is_err() {
        return crate::models::ExecutableArchitecture::Unknown;
    }
    let pe_offset = u32::from_le_bytes(pe_offset_buf);

    if file.seek(SeekFrom::Start(pe_offset as u64)).is_err() {
        return crate::models::ExecutableArchitecture::Unknown;
    }

    let mut pe_signature = [0u8; 4];
    if file.read_exact(&mut pe_signature).is_err() || &pe_signature != b"PE\0\0" {
        return crate::models::ExecutableArchitecture::Unknown;
    }

    // COFF Header starts right after PE signature
    // Machine is the first 2 bytes
    let mut machine_buf = [0u8; 2];
    if file.read_exact(&mut machine_buf).is_err() {
        return crate::models::ExecutableArchitecture::Unknown;
    }
    let machine = u16::from_le_bytes(machine_buf);

    match machine {
        0x014c => crate::models::ExecutableArchitecture::X86,
        0x8664 => crate::models::ExecutableArchitecture::X86_64,
        _ => crate::models::ExecutableArchitecture::Unknown,
    }
}

pub fn detect_custom_components(path: &Path) -> crate::utils::RunnerComponents {
    let (dxvk, vkd3d_proton, vkd3d, nvapi) = (
        detect_dxvk(path, None),
        detect_vkd3d_proton(path, None),
        detect_vkd3d(path, None),
        detect_nvapi(path, None),
    );

    crate::utils::RunnerComponents {
        dxvk,
        vkd3d_proton,
        vkd3d,
        nvapi,
    }
}

pub fn deploy_dll_symlinks(
    prefix: &Path,
    resolutions: &[crate::launch::dll_provider_resolver::DllResolution],
    target_arch: &crate::models::ExecutableArchitecture,
) -> Result<Vec<PathBuf>> {
    let mut deployed = Vec::new();
    let is_64bit_prefix = prefix.join("drive_c/windows/syswow64").exists();

    for res in resolutions {
        if res.chosen_provider != crate::launch::dll_provider_resolver::DllProvider::Runner &&
           res.chosen_provider != crate::launch::dll_provider_resolver::DllProvider::Custom {
            continue;
        }

        if let Some(src_path) = &res.chosen_path {
            let dll_name = format!("{}.dll", res.name);

            // Determine destination directory in prefix
            let dest_dir = match target_arch {
                crate::models::ExecutableArchitecture::X86_64 => {
                    prefix.join("drive_c/windows/system32")
                }
                crate::models::ExecutableArchitecture::X86 => {
                    if is_64bit_prefix {
                        prefix.join("drive_c/windows/syswow64")
                    } else {
                        prefix.join("drive_c/windows/system32")
                    }
                }
                _ => continue,
            };

            if !dest_dir.exists() {
                continue;
            }

            let dest_path = dest_dir.join(&dll_name);

            // Safety check: if it exists and is not a symlink, back it up or skip?
            // Usually we want to replace it if it's a Wine builtin.
            if dest_path.exists() {
                let meta = std::fs::symlink_metadata(&dest_path)?;
                if !meta.file_type().is_symlink() {
                    let backup = dest_path.with_extension("dll.bak");
                    if !backup.exists() {
                        tracing::info!("Backing up original DLL: {} -> {}", dest_path.display(), backup.display());
                        std::fs::rename(&dest_path, &backup)?;
                    } else {
                        // Backup already exists, just remove the original to make room for symlink
                        std::fs::remove_file(&dest_path)?;
                    }
                } else {
                    // It's already a symlink, remove it to update
                    std::fs::remove_file(&dest_path)?;
                }
            }

            tracing::info!("Symlinking {} -> {}", src_path.display(), dest_path.display());
            #[cfg(unix)]
            std::os::unix::fs::symlink(src_path, &dest_path)?;
            #[cfg(not(unix))]
            std::fs::copy(src_path, &dest_path)?;

            deployed.push(dest_path);

            // Also try to deploy the "other" architecture if it's a 64-bit prefix and we have it
            if is_64bit_prefix {
                let (other_arch, other_dir) = match target_arch {
                    crate::models::ExecutableArchitecture::X86_64 => (
                        crate::models::ExecutableArchitecture::X86,
                        prefix.join("drive_c/windows/syswow64")
                    ),
                    crate::models::ExecutableArchitecture::X86 => (
                        crate::models::ExecutableArchitecture::X86_64,
                        prefix.join("drive_c/windows/system32")
                    ),
                    _ => continue,
                };

                // We need to find the sibling DLL.
                // This is a bit tricky because we don't have the full resolution for the other arch here.
                // But we can guess based on common layouts.
                if let Some(other_src) = find_sibling_dll(src_path, target_arch, &other_arch) {
                    let other_dest = other_dir.join(&dll_name);
                    if other_dest.exists() {
                        let meta = std::fs::symlink_metadata(&other_dest)?;
                        if !meta.file_type().is_symlink() {
                            let backup = other_dest.with_extension("dll.bak");
                            if !backup.exists() {
                                std::fs::rename(&other_dest, &backup)?;
                            } else {
                                std::fs::remove_file(&other_dest)?;
                            }
                        } else {
                            std::fs::remove_file(&other_dest)?;
                        }
                    }
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(&other_src, &other_dest)?;
                    #[cfg(not(unix))]
                    std::fs::copy(&other_src, &other_dest)?;
                    deployed.push(other_dest);
                }
            }
        }
    }

    Ok(deployed)
}

fn find_sibling_dll(
    path: &Path,
    current_arch: &crate::models::ExecutableArchitecture,
    target_arch: &crate::models::ExecutableArchitecture,
) -> Option<PathBuf> {
    let (current_tag, target_tag) = match (current_arch, target_arch) {
        (crate::models::ExecutableArchitecture::X86_64, crate::models::ExecutableArchitecture::X86) => ("x86_64", "i386"),
        (crate::models::ExecutableArchitecture::X86, crate::models::ExecutableArchitecture::X86_64) => ("i386", "x86_64"),
        _ => return None,
    };

    let path_str = path.to_string_lossy();
    if path_str.contains(current_tag) {
        let other_str = path_str.replace(current_tag, target_tag);
        let other_path = PathBuf::from(other_str);
        if other_path.exists() {
            return Some(other_path);
        }
    }

    // Also check for x64/x32 variant
    let (current_tag2, target_tag2) = match (current_arch, target_arch) {
        (crate::models::ExecutableArchitecture::X86_64, crate::models::ExecutableArchitecture::X86) => ("x64", "x32"),
        (crate::models::ExecutableArchitecture::X86, crate::models::ExecutableArchitecture::X86_64) => ("x32", "x64"),
        _ => return None,
    };
    if path_str.contains(current_tag2) {
        let other_str = path_str.replace(current_tag2, target_tag2);
        let other_path = PathBuf::from(other_str);
        if other_path.exists() {
            return Some(other_path);
        }
    }

    None
}

pub fn cleanup_dll_symlinks(prefix: &Path) -> Result<()> {
    let target_dlls = [
        "d3d8.dll", "d3d9.dll", "dxgi.dll", "d3d10core.dll",
        "d3d11.dll", "d3d12.dll", "d3d12core.dll", "libvkd3d-1.dll", "libvkd3d-shader-1.dll"
    ];

    let dirs = [
        prefix.join("drive_c/windows/system32"),
        prefix.join("drive_c/windows/syswow64"),
    ];

    for dir in dirs {
        if !dir.exists() { continue; }
        for dll in &target_dlls {
            let p = dir.join(dll);
            if p.exists() {
                let meta = std::fs::symlink_metadata(&p)?;
                if meta.file_type().is_symlink() {
                    tracing::info!("Cleaning up symlink: {}", p.display());
                    std::fs::remove_file(&p)?;

                    // Restore backup if it exists
                    let backup = p.with_extension("dll.bak");
                    if backup.exists() {
                        tracing::info!("Restoring backup: {} -> {}", backup.display(), p.display());
                        std::fs::rename(&backup, &p)?;
                    }
                }
            }
        }
    }

    Ok(())
}

pub fn steam_wineprefix_for_game(
    config: &crate::config::LauncherConfig,
    app_id: u32,
    user_configs: &crate::models::UserConfigStore,
) -> std::path::PathBuf {
    let use_steam_runtime = match user_configs.get(&app_id).map(|c| &c.steam_runtime_policy) {
        Some(crate::models::SteamRuntimePolicy::Enabled) => true,
        Some(crate::models::SteamRuntimePolicy::Disabled) => false,
        Some(crate::models::SteamRuntimePolicy::Auto) | None => {
            user_configs.get(&app_id).map(|c| c.use_steam_runtime).unwrap_or(false)
        }
    };

    let use_per_game_compat_data = user_configs.get(&app_id)
        .map(|c| use_steam_runtime && c.steam_prefix_mode == crate::models::SteamPrefixMode::PerGame)
        .unwrap_or(config.use_shared_compat_data);

    if use_per_game_compat_data {
        std::path::PathBuf::from(&config.steam_library_path)
            .join("steamapps")
            .join("compatdata")
            .join(app_id.to_string())
            .join("pfx")
    } else {
        resolve_master_wineprefix()
    }
}
