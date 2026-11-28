//! Filesystem helpers.

use std::path::Path;

use anyhow::Result;

use walkdir::WalkDir;

/// Free bytes on `path`'s filesystem.
pub fn available_space(path: &Path) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list().ok()?;
    disks
        .iter()
        .filter(|disk| path.starts_with(disk.mount_point()))
        // Longest matching mount point wins.
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| disk.available_space())
}

/// Regular-file bytes under `path`; 0 if missing.
pub fn dir_size(path: &Path) -> u64 {
    WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Writes `steam_appid.txt` only when missing.
pub fn ensure_steam_appid(dir: &Path, app_id: u32) -> std::io::Result<bool> {
    let path = dir.join("steam_appid.txt");
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => {
            write_appid_or_remove(&path, file, app_id)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

/// Writes; removes `path` on failure.
fn write_appid_or_remove(
    path: &Path,
    mut file: impl std::io::Write,
    app_id: u32,
) -> std::io::Result<()> {
    let result = file
        .write_all(app_id.to_string().as_bytes())
        .and_then(|()| file.flush());
    drop(file);
    if let Err(e) = result {
        // Partial file blocks later retries.
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(())
}

/// [`ensure_steam_appid`] that logs instead of failing.
pub fn ensure_steam_appid_logged(dir: &Path, app_id: u32) {
    match ensure_steam_appid(dir, app_id) {
        Ok(true) => tracing::debug!("wrote steam_appid.txt in {}", dir.display()),
        Ok(false) => tracing::debug!("keeping existing steam_appid.txt in {}", dir.display()),
        Err(e) => tracing::warn!("could not write steam_appid.txt in {}: {e}", dir.display()),
    }
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

#[cfg(test)]
mod tests;
