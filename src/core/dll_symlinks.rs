//! DLL symlink deployment into prefixes.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Place a single DLL at `dest` as a symlink to `src` (a plain copy on non-unix),
/// first clearing any existing entry: a real (non-symlink) file is moved aside to
/// `*.dll.bak` (or removed if a backup already exists), and a stale symlink is
/// removed. When `log` is set the backup and symlink steps emit info-level traces;
/// the sibling-arch deploy passes `log = false` to stay quiet, preserving prior
/// behavior exactly.
fn deploy_one_dll_symlink(src: &Path, dest: &Path, log: bool) -> Result<()> {
    // Safety check: if it exists and is not a symlink, back it up or skip?
    // Usually we want to replace it if it's a Wine builtin.
    if dest.exists() {
        let meta = std::fs::symlink_metadata(dest)?;
        if !meta.file_type().is_symlink() {
            let backup = dest.with_extension("dll.bak");
            if !backup.exists() {
                if log {
                    tracing::info!(
                        "Backing up original DLL: {} -> {}",
                        dest.display(),
                        backup.display()
                    );
                }
                std::fs::rename(dest, &backup)?;
            } else {
                // Backup already exists, just remove the original to make room for symlink
                std::fs::remove_file(dest)?;
            }
        } else {
            // It's already a symlink, remove it to update
            std::fs::remove_file(dest)?;
        }
    }

    if log {
        tracing::info!("Symlinking {} -> {}", src.display(), dest.display());
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(src, dest)?;
    #[cfg(not(unix))]
    std::fs::copy(src, dest)?;

    Ok(())
}

pub fn deploy_dll_symlinks(
    prefix: &Path,
    resolutions: &[crate::launch::dll_provider_resolver::DllResolution],
    target_arch: &crate::core::models::ExecutableArchitecture,
) -> Result<Vec<PathBuf>> {
    let mut deployed = Vec::new();
    let is_64bit_prefix = prefix.join("drive_c/windows/syswow64").exists();

    for res in resolutions {
        if res.chosen_provider != crate::launch::dll_provider_resolver::DllProvider::Runner
            && res.chosen_provider != crate::launch::dll_provider_resolver::DllProvider::Custom
        {
            continue;
        }

        if let Some(src_path) = &res.chosen_path {
            let dll_name = format!("{}.dll", res.name);

            // Determine destination directory in prefix
            let dest_dir = match target_arch {
                crate::core::models::ExecutableArchitecture::X86_64 => {
                    prefix.join("drive_c/windows/system32")
                }
                crate::core::models::ExecutableArchitecture::X86 => {
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

            deploy_one_dll_symlink(src_path, &dest_path, true)?;

            deployed.push(dest_path);

            // Also try to deploy the "other" architecture if it's a 64-bit prefix and we have it
            if is_64bit_prefix {
                let (other_arch, other_dir) = match target_arch {
                    crate::core::models::ExecutableArchitecture::X86_64 => (
                        crate::core::models::ExecutableArchitecture::X86,
                        prefix.join("drive_c/windows/syswow64"),
                    ),
                    crate::core::models::ExecutableArchitecture::X86 => (
                        crate::core::models::ExecutableArchitecture::X86_64,
                        prefix.join("drive_c/windows/system32"),
                    ),
                    _ => continue,
                };

                // We need to find the sibling DLL.
                // This is a bit tricky because we don't have the full resolution for the other arch here.
                // But we can guess based on common layouts.
                if let Some(other_src) = find_sibling_dll(src_path, target_arch, &other_arch) {
                    let other_dest = other_dir.join(&dll_name);
                    deploy_one_dll_symlink(&other_src, &other_dest, false)?;
                    deployed.push(other_dest);
                }
            }
        }
    }

    Ok(deployed)
}

fn find_sibling_dll(
    path: &Path,
    current_arch: &crate::core::models::ExecutableArchitecture,
    target_arch: &crate::core::models::ExecutableArchitecture,
) -> Option<PathBuf> {
    use crate::core::models::ExecutableArchitecture as Arch;
    let flip = match (current_arch, target_arch) {
        (Arch::X86_64, Arch::X86) => false,
        (Arch::X86, Arch::X86_64) => true,
        _ => return None,
    };

    let path_str = path.to_string_lossy();
    for (tag64, tag32) in [("x86_64", "i386"), ("x64", "x32")] {
        let (current_tag, target_tag) = if flip { (tag32, tag64) } else { (tag64, tag32) };
        if path_str.contains(current_tag) {
            let other_path = PathBuf::from(path_str.replace(current_tag, target_tag));
            if other_path.exists() {
                return Some(other_path);
            }
        }
    }

    None
}

pub fn cleanup_dll_symlinks(prefix: &Path) -> Result<()> {
    let target_dlls = [
        "d3d8.dll",
        "d3d9.dll",
        "dxgi.dll",
        "d3d10core.dll",
        "d3d11.dll",
        "d3d12.dll",
        "d3d12core.dll",
        "libvkd3d-1.dll",
        "libvkd3d-shader-1.dll",
    ];

    let dirs = [
        prefix.join("drive_c/windows/system32"),
        prefix.join("drive_c/windows/syswow64"),
    ];

    for dir in dirs {
        if !dir.exists() {
            continue;
        }
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
