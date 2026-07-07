use std::path::{Path, PathBuf};
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

/// Whether a relative discovery path is *unambiguously* 64-bit: it carries an
/// `x86_64` arch marker (covers `x86_64-windows` and the WOW64 `x86_64-unix` bridge)
/// or lives under a `lib64` segment. Paths carrying an `i386` marker are never 64-bit.
fn is_definitely_64bit(p: &str) -> bool {
    !p.contains("i386") && (p.contains("x86_64") || p.contains("lib64"))
}

/// Whether a relative discovery path is *unambiguously* 32-bit: it carries an `i386`
/// arch marker (covers `i386-windows` and the WOW64 `i386-unix` bridge).
fn is_definitely_32bit(p: &str) -> bool {
    p.contains("i386")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentScanReport {
    pub runner_binary: PathBuf,
    pub runner_root: PathBuf,
    pub scan_roots: Vec<PathBuf>,
    pub components_found: HashMap<String, ComponentFoundInfo>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentFoundInfo {
    pub family: String,
    pub version: String,
    pub source: String,
    pub matched_dll: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DllProvider {
    GameLocal,
    Custom,
    Runner,
    System,
    Internal, // Satisfied via capability (e.g. DXVK D3D10 core)
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DllResolution {
    pub name: String,
    pub chosen_provider: DllProvider,
    pub chosen_path: Option<PathBuf>,
    pub fallback_reason: Option<String>,
    pub candidates: Vec<DllCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DllCandidate {
    pub provider: DllProvider,
    pub path: PathBuf,
    pub exists: bool,
}

pub struct DllProviderResolver {
    target_dlls: Vec<String>,
}

impl DllProviderResolver {
    pub fn new() -> Self {
        Self {
            target_dlls: vec![
                "d3d8".into(),
                "d3d9".into(),
                "dxgi".into(),
                "d3d10core".into(),
                "d3d11".into(),
                "d3d12".into(),
                "d3d12core".into(),
                "libvkd3d-1".into(),
                "libvkd3d-shader-1".into(),
                "nvapi".into(),
                "nvapi64".into(),
                "nvofapi64".into(),
            ],
        }
    }

    pub fn resolve(
        &self,
        game_exe_dir: &Path,
        runner_path: &Path,
        runner_components: &crate::utils::RunnerComponents,
        d3d12_policy: &crate::models::D3D12ProviderPolicy,
        target_arch: &crate::models::ExecutableArchitecture,
        custom_dxvk_path: Option<&Path>,
        custom_vkd3d_path: Option<&Path>,
        custom_vkd3d_proton_path: Option<&Path>,
    ) -> (Vec<DllResolution>, ComponentScanReport) {
        tracing::debug!("Resolving DLL providers. ExeDir: {}, Runner: {}", game_exe_dir.display(), runner_path.display());
        let runner_root = crate::utils::derive_runner_root(runner_path);

        let mut report = ComponentScanReport {
            runner_binary: runner_path.to_path_buf(),
            runner_root: runner_root.clone(),
            scan_roots: Vec::new(),
            components_found: HashMap::new(),
            warnings: Vec::new(),
        };

        if runner_root == PathBuf::from(".") || runner_root.to_string_lossy().is_empty() {
             report.warnings.push("Runner root derivation failed or resulted in empty path".into());
        } else {
             // Derive all potential runner scan roots from the shared unified-layout
             // constants (component × lib-root × arch, plus bare component dirs).
             report.scan_roots = crate::proton::COMPONENT_FAMILIES
                 .iter()
                 .flat_map(|fam| crate::proton::component_dll_subdirs(fam))
                 .map(|s| runner_root.join(s))
                 .collect();
        }

        let resolutions: Vec<DllResolution> = self.target_dlls
            .iter()
            .map(|dll| self.resolve_single(
                dll,
                game_exe_dir,
                runner_path,
                runner_components,
                d3d12_policy,
                target_arch,
                custom_dxvk_path,
                custom_vkd3d_path,
                custom_vkd3d_proton_path,
            ))
            .collect();

        let mut record_component = |family: &str, component: &Option<crate::utils::ComponentInfo>| {
            if let Some(c) = component {
                report.components_found.insert(family.into(), ComponentFoundInfo {
                    family: family.into(),
                    version: c.version.clone(),
                    source: format!("{:?}", c.source),
                    matched_dll: None,
                });
            }
        };
        record_component("dxvk", &runner_components.dxvk);
        record_component("nvapi", &runner_components.nvapi);
        record_component("vkd3d-proton", &runner_components.vkd3d_proton);
        record_component("vkd3d", &runner_components.vkd3d);

        for res in &resolutions {
            let game_local_count = res.candidates.iter().filter(|c| c.provider == DllProvider::GameLocal && c.exists).count();
            let runner_count = res.candidates.iter().filter(|c| c.provider == DllProvider::Runner && c.exists).count();
            let system_count = res.candidates.iter().filter(|c| c.provider == DllProvider::System && c.exists).count();

            tracing::debug!(
                "DLL {}: chosen={:?} (candidates: GameLocal={}, Runner={}, System={})",
                res.name, res.chosen_provider, game_local_count, runner_count, system_count
            );

            if res.chosen_provider == DllProvider::Runner {
                if let Some(path) = &res.chosen_path {
                     // Try to match back to component
                     let family = if res.name.starts_with("d3d12") || res.name.contains("vkd3d") {
                         if path.to_string_lossy().contains("vkd3d-proton") { "vkd3d-proton" } else { "vkd3d" }
                     } else if res.name.contains("nvapi") {
                         "nvapi"
                     } else {
                         "dxvk"
                     };
                     if let Some(info) = report.components_found.get_mut(family) {
                         info.matched_dll = Some(path.clone());
                     }
                }
            }
        }

        (resolutions, report)
    }

    fn resolve_single(
        &self,
        dll_name: &str,
        game_exe_dir: &Path,
        runner_path: &Path,
        runner_components: &crate::utils::RunnerComponents,
        d3d12_policy: &crate::models::D3D12ProviderPolicy,
        target_arch: &crate::models::ExecutableArchitecture,
        custom_dxvk_path: Option<&Path>,
        custom_vkd3d_path: Option<&Path>,
        custom_vkd3d_proton_path: Option<&Path>,
    ) -> DllResolution {
        let mut candidates = Vec::new();
        let dll_filename = format!("{}.dll", dll_name);

        // 1. GameLocal Priority
        let local_path = game_exe_dir.join(&dll_filename);
        candidates.push(DllCandidate {
            provider: DllProvider::GameLocal,
            path: local_path.clone(),
            exists: local_path.exists(),
        });

        // Also check 'bin' subdir common in some games
        let bin_path = game_exe_dir.join("bin").join(&dll_filename);
        candidates.push(DllCandidate {
            provider: DllProvider::GameLocal,
            path: bin_path.clone(),
            exists: bin_path.exists(),
        });

        // 2. Custom Path Priority
        if let Some(path) = self.get_custom_dll_path(
            dll_name,
            target_arch,
            custom_dxvk_path,
            custom_vkd3d_path,
            custom_vkd3d_proton_path,
        ) {
            candidates.push(DllCandidate {
                provider: DllProvider::Custom,
                path: path.clone(),
                exists: path.exists(),
            });
        }

        // 3. Runner Priority
        if let Some(path) = self.get_runner_dll_path(dll_name, runner_path, runner_components, d3d12_policy, target_arch) {
            candidates.push(DllCandidate {
                provider: DllProvider::Runner,
                path: path.clone(),
                exists: path.exists(),
            });
        }

        // 3. System Priority
        // For now, we use a simplified check for system paths
        let system_paths = match dll_name {
            "d3d8" | "d3d9" | "d3d10core" | "d3d11" | "dxgi" => vec![
                "/usr/lib/dxvk/x64",
                "/usr/lib/x86_64-linux-gnu/dxvk",
            ],
            "d3d12" | "d3d12core" | "libvkd3d-1" | "libvkd3d-shader-1" => vec![
                "/usr/lib/vkd3d-proton/x64",
                "/usr/lib/x86_64-linux-gnu/vkd3d-proton",
                "/usr/lib/x86_64-linux-gnu", // standard system vkd3d
                "/usr/lib64",
            ],
            "nvapi" | "nvapi64" | "nvofapi64" => vec![
                "/usr/lib/nvapi/x64",
                "/usr/lib/x86_64-linux-gnu/nvapi",
            ],
            _ => vec![],
        };

        for base in system_paths {
            let p = Path::new(base).join(&dll_filename);
            candidates.push(DllCandidate {
                provider: DllProvider::System,
                path: p.clone(),
                exists: p.exists(),
            });
        }

        let chosen = candidates.iter().find(|c| c.exists).cloned();
        let fallback_reason = chosen.is_none().then(||
            "no candidate files found in GameLocal, Custom, Runner, or System paths".to_string()
        );

        DllResolution {
            name: dll_name.to_string(),
            chosen_provider: chosen.as_ref().map_or(DllProvider::None, |c| c.provider),
            chosen_path: chosen.as_ref().map(|c| c.path.clone()),
            fallback_reason,
            candidates,
        }
    }

    fn get_custom_dll_path(
        &self,
        dll_name: &str,
        target_arch: &crate::models::ExecutableArchitecture,
        custom_dxvk_path: Option<&Path>,
        custom_vkd3d_path: Option<&Path>,
        custom_vkd3d_proton_path: Option<&Path>,
    ) -> Option<PathBuf> {
        let dll_filename = format!("{}.dll", dll_name);
        let is_dxvk = matches!(dll_name, "d3d8" | "d3d9" | "d3d10core" | "d3d11" | "dxgi");
        let is_vkd3d_proton = matches!(dll_name, "d3d12" | "d3d12core");
        let is_vkd3d = matches!(dll_name, "libvkd3d-1" | "libvkd3d-shader-1");

        let custom_root = if is_dxvk {
            custom_dxvk_path
        } else if is_vkd3d_proton {
            custom_vkd3d_proton_path
        } else if is_vkd3d {
            custom_vkd3d_path
        } else {
            None
        };

        if let Some(root) = custom_root {
            let mut relative_paths = vec![
                "x86_64-windows",
                "i386-windows",
                "x64",
                "x32",
                "",
            ];

            // Strictly filter by architecture
            match target_arch {
                crate::models::ExecutableArchitecture::X86 => {
                    relative_paths.retain(|p| p.contains("i386") || p.contains("x32") || p.is_empty());
                }
                crate::models::ExecutableArchitecture::X86_64 => {
                    relative_paths.retain(|p| p.contains("x86_64") || p.contains("x64") || p.is_empty());
                }
                _ => {}
            }

            for rel in relative_paths {
                let p = root.join(rel).join(&dll_filename);
                if p.exists() {
                    return Some(p);
                }
            }
        }

        None
    }

    /// Filters candidate relative paths by architecture, then returns the first
    /// `<runner_root>/<rel>/<dll_filename>` that exists on disk.
    fn find_in_runner_paths(
        runner_root: &Path,
        dll_filename: &str,
        target_arch: &crate::models::ExecutableArchitecture,
        relative_paths: Vec<String>,
    ) -> Option<PathBuf> {
        let mut relative_paths = relative_paths;
        match target_arch {
            // Exclude anything that is *unambiguously* the wrong bitness. Bare
            // component dirs (no arch marker, no `lib64` segment) stay arch-neutral
            // so legacy split-lib and flat unified layouts both keep working.
            crate::models::ExecutableArchitecture::X86 => {
                relative_paths.retain(|p| !is_definitely_64bit(p));
            }
            crate::models::ExecutableArchitecture::X86_64 => {
                relative_paths.retain(|p| !is_definitely_32bit(p));
            }
            // Unknown architecture: keep the conservative legacy behavior of trying
            // every candidate (no bitness filter) rather than guessing and dropping a
            // path that would otherwise resolve.
            _ => {}
        }

        for rel in relative_paths {
            let p = runner_root.join(&rel).join(dll_filename);
            if p.exists() {
                tracing::trace!("Found runner component DLL at: {}", p.display());
                return Some(p);
            }
        }
        None
    }

    fn get_runner_dll_path(
        &self,
        dll_name: &str,
        runner_path: &Path,
        components: &crate::utils::RunnerComponents,
        d3d12_policy: &crate::models::D3D12ProviderPolicy,
        target_arch: &crate::models::ExecutableArchitecture,
    ) -> Option<PathBuf> {
        let runner_root = crate::utils::derive_runner_root(runner_path);

        let dll_filename = format!("{}.dll", dll_name);

        // Match DLL to component and look for it in runner root. The candidate lists
        // are composed from the shared unified-layout constants (component × lib-root
        // × arch, plus bare component dirs), so they cover Proton 11+/GE/CachyOS.
        let is_dxvk = matches!(dll_name, "d3d8" | "d3d9" | "d3d10core" | "d3d11" | "dxgi");
        if is_dxvk && components.dxvk.is_some() {
            if let Some(p) = Self::find_in_runner_paths(&runner_root, &dll_filename, target_arch,
                crate::proton::component_dll_subdirs("dxvk")) {
                return Some(p);
            }
        }

        let is_nvapi = matches!(dll_name, "nvapi" | "nvapi64" | "nvofapi64");
        if is_nvapi && components.nvapi.is_some() {
            if let Some(p) = Self::find_in_runner_paths(&runner_root, &dll_filename, target_arch,
                crate::proton::component_dll_subdirs("nvapi")) {
                return Some(p);
            }
        }

        let is_vkd3d_any = matches!(dll_name, "d3d12" | "d3d12core" | "libvkd3d-1" | "libvkd3d-shader-1");
        if is_vkd3d_any {
            let use_proton = matches!(
                d3d12_policy,
                crate::models::D3D12ProviderPolicy::Auto | crate::models::D3D12ProviderPolicy::Vkd3dProton
            );

            if use_proton && components.vkd3d_proton.is_some() {
                if let Some(p) = Self::find_in_runner_paths(&runner_root, &dll_filename, target_arch,
                    crate::proton::component_dll_subdirs("vkd3d-proton")) {
                    return Some(p);
                }
            }

            if (!use_proton || d3d12_policy == &crate::models::D3D12ProviderPolicy::Auto) && components.vkd3d.is_some() {
                if let Some(p) = Self::find_in_runner_paths(&runner_root, &dll_filename, target_arch,
                    crate::proton::component_dll_subdirs("vkd3d")) {
                    return Some(p);
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

    #[test]
    fn test_dll_priority_game_local() {
        let tmp = tempdir().unwrap();
        let game_dir = tmp.path().to_path_buf();
        let d3d9_dll = game_dir.join("d3d9.dll");
        fs::write(&d3d9_dll, "local content").unwrap();

        let resolver = DllProviderResolver::new();
        let runner_path = Path::new("/tmp/fake_runner");
        let components = crate::utils::RunnerComponents::default();
        let d3d12_policy = crate::models::D3D12ProviderPolicy::Auto;
        let arch = crate::models::ExecutableArchitecture::X86_64;
        let (resolutions, _) = resolver.resolve(&game_dir, runner_path, &components, &d3d12_policy, &arch, None, None, None);

        let d3d9_res = resolutions.iter().find(|r| r.name == "d3d9").unwrap();
        assert_eq!(d3d9_res.chosen_provider, DllProvider::GameLocal);
        assert_eq!(d3d9_res.chosen_path.as_ref().unwrap(), &d3d9_dll);
    }

    #[test]
    fn test_dll_priority_system_fallback() {
        // We can't easily test system paths because they are absolute and might not exist
        // But we can verify the logic correctly identifies 'None' when no tier matches.
        let tmp = tempdir().unwrap();
        let game_dir = tmp.path().to_path_buf();

        let resolver = DllProviderResolver::new();
        let runner_path = Path::new("/tmp/fake_runner");
        let components = crate::utils::RunnerComponents::default();
        let d3d12_policy = crate::models::D3D12ProviderPolicy::Auto;
        let arch = crate::models::ExecutableArchitecture::X86_64;
        let (resolutions, _) = resolver.resolve(&game_dir, runner_path, &components, &d3d12_policy, &arch, None, None, None);

        for res in resolutions {
            if res.chosen_provider == DllProvider::System {
                // OK if system has them
            } else {
                assert_eq!(res.chosen_provider, DllProvider::None);
            }
        }
    }

    #[test]
    fn test_d3d12_provider_selection() {
        let tmp = tempdir().unwrap();
        let runner_root = tmp.path().to_path_buf();
        let proton_dir = runner_root.join("files/lib/wine/vkd3d-proton");
        let wine_dir = runner_root.join("files/lib/wine/vkd3d");
        fs::create_dir_all(&proton_dir).unwrap();
        fs::create_dir_all(&wine_dir).unwrap();

        let proton_dll = proton_dir.join("d3d12.dll");
        let wine_dll = wine_dir.join("d3d12.dll");
        fs::write(&proton_dll, "proton").unwrap();
        fs::write(&wine_dll, "wine").unwrap();

        let mut components = crate::utils::RunnerComponents::default();
        components.vkd3d_proton = Some(crate::utils::ComponentInfo {
            version: "2.10".into(),
            source: crate::utils::ComponentSource::BundledWithRunner,
            path: None,
        });
        components.vkd3d = Some(crate::utils::ComponentInfo {
            version: "1.8".into(),
            source: crate::utils::ComponentSource::BundledWithRunner,
            path: None,
        });

        let resolver = DllProviderResolver::new();
        let game_dir = Path::new("/tmp/game");
        let arch = crate::models::ExecutableArchitecture::X86_64;

        // Case 1: Auto (Prefer Proton)
        let (res, _) = resolver.resolve(game_dir, &runner_root, &components, &crate::models::D3D12ProviderPolicy::Auto, &arch, None, None, None);
        let d3d12 = res.iter().find(|r| r.name == "d3d12").unwrap();
        assert_eq!(d3d12.chosen_path.as_ref().unwrap(), &proton_dll);

        // Case 2: Explicit Wine
        let (res, _) = resolver.resolve(game_dir, &runner_root, &components, &crate::models::D3D12ProviderPolicy::Vkd3dWine, &arch, None, None, None);
        let d3d12 = res.iter().find(|r| r.name == "d3d12").unwrap();
        assert_eq!(d3d12.chosen_path.as_ref().unwrap(), &wine_dll);

        // Case 3: Explicit Proton
        let (res, _) = resolver.resolve(game_dir, &runner_root, &components, &crate::models::D3D12ProviderPolicy::Vkd3dProton, &arch, None, None, None);
        let d3d12 = res.iter().find(|r| r.name == "d3d12").unwrap();
        assert_eq!(d3d12.chosen_path.as_ref().unwrap(), &proton_dll);
    }

    #[test]
    fn test_d3d8_coverage() {
        let resolver = DllProviderResolver::new();
        assert!(resolver.target_dlls.contains(&"d3d8".to_string()));
    }

    #[test]
    fn test_fallback_reason_populated() {
        let resolver = DllProviderResolver::new();
        let tmp = tempdir().unwrap();
        let arch = crate::models::ExecutableArchitecture::X86_64;
        let (res, _) = resolver.resolve(tmp.path(), tmp.path(), &crate::utils::RunnerComponents::default(), &crate::models::D3D12ProviderPolicy::Auto, &arch, None, None, None);
        let d3d11 = res.iter().find(|r| r.name == "d3d11").unwrap();
        assert_eq!(d3d11.chosen_provider, DllProvider::None);
        assert!(d3d11.fallback_reason.is_some());
    }
}
