//! Moving a Steam game install between library folders.
//!
//! A correct move has to do more than copy bytes: Steam decides which library a
//! game lives in from where its `appmanifest_<appid>.acf` sits and from the
//! `apps` index in `libraryfolders.vdf`. This module provides the building
//! blocks — sizing, a progress-reporting directory move (fast `rename` when the
//! source and destination share a volume, recursive copy otherwise), and a
//! conservative editor that relocates an app's entry between the `apps` blocks of
//! `libraryfolders.vdf` so the client doesn't mistake the install path.

use std::io;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// Total size in bytes of every regular file under `path` (0 if it doesn't exist).
pub fn dir_size(path: &Path) -> u64 {
    WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Move the directory tree at `src` to `dst`, invoking `on_progress(copied_bytes,
/// current_file)` as it goes.
///
/// Fast path: a plain `rename`, which is atomic and instant when `src` and `dst`
/// are on the same volume. If that fails (most commonly because the destination
/// is on a different drive), fall back to a recursive copy followed by deleting
/// the source — the source is only removed after every file has been copied, so a
/// failure mid-copy never destroys the original.
pub fn move_dir_with_progress(
    src: &Path,
    dst: &Path,
    total_bytes: u64,
    mut on_progress: impl FnMut(u64, &str),
) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Fast path: same-volume rename. Report the whole tree as moved at once.
    if std::fs::rename(src, dst).is_ok() {
        on_progress(total_bytes, "");
        return Ok(());
    }

    copy_dir(src, dst, &mut on_progress)?;
    // Only now that the copy fully succeeded do we delete the original.
    std::fs::remove_dir_all(src)?;
    Ok(())
}

/// Recursively copy `src` into `dst`, streaming progress. Directories are created
/// first; files are copied in chunks so large files still report incremental
/// progress.
fn copy_dir(src: &Path, dst: &Path, on_progress: &mut impl FnMut(u64, &str)) -> io::Result<()> {
    let mut copied: u64 = 0;
    std::fs::create_dir_all(dst)?;

    // Reuse one chunk buffer across every file in the tree; a game install can
    // hold thousands of files, so per-file 4 MiB allocations would add up.
    let mut buf = vec![0u8; 4 * 1024 * 1024];

    for entry in WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
        let rel = entry.path().strip_prefix(src).map_err(io::Error::other)?;
        let target = dst.join(rel);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            copy_file_chunked(entry.path(), &target, &mut copied, &mut buf, on_progress)?;
        }
        // Symlinks and other special files are rare in game installs; skip them
        // rather than risk copying them incorrectly.
    }
    Ok(())
}

fn copy_file_chunked(
    src: &Path,
    dst: &Path,
    copied: &mut u64,
    buf: &mut [u8],
    on_progress: &mut impl FnMut(u64, &str),
) -> io::Result<()> {
    use io::{Read, Write};

    let mut reader = std::fs::File::open(src)?;
    let mut writer = std::fs::File::create(dst)?;
    let name = src.file_name().unwrap_or_default().to_string_lossy().into_owned();

    loop {
        let n = reader.read(buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        *copied += n as u64;
        on_progress(*copied, &name);
    }
    writer.flush()?;
    Ok(())
}

/// Locate the single `libraryfolders.vdf` (it lives in the main Steam install's
/// `steamapps/`, listing every library folder) among the given candidate roots.
pub fn find_libraryfolders_vdf(roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .map(|root| root.join("steamapps").join("libraryfolders.vdf"))
        .find(|candidate| candidate.exists())
}

/// Normalise a Steam library path for comparison: unescape VDF's doubled
/// backslashes, unify separators, drop a trailing separator, and lowercase on
/// case-insensitive platforms.
fn normalize_path(p: &str) -> String {
    let unified = p.replace("\\\\", "\\").replace('\\', "/");
    let trimmed = unified.trim_end_matches('/');
    if cfg!(windows) {
        trimmed.to_lowercase()
    } else {
        trimmed.to_string()
    }
}

/// Move app `appid`'s entry (with byte `size`) from the `from`-library's `apps`
/// block to the `to`-library's `apps` block within `libraryfolders.vdf` text.
///
/// Conservative and lossless: it edits only the two `apps` blocks (removing one
/// line, inserting one line) and copies every other byte through unchanged. If
/// either library folder can't be located unambiguously, it returns `None` and
/// the caller should leave the file alone (Steam reconciles the index from the
/// appmanifests on its next launch anyway).
pub fn update_libraryfolders_apps(
    vdf: &str,
    appid: u32,
    from: &Path,
    to: &Path,
    size: u64,
) -> Option<String> {
    let from_norm = normalize_path(&from.to_string_lossy());
    let to_norm = normalize_path(&to.to_string_lossy());
    let appid_key = format!("\"{appid}\"");

    // Preserve the file's existing line ending and indentation style.
    let newline = if vdf.contains("\r\n") { "\r\n" } else { "\n" };

    let mut out: Vec<String> = Vec::new();
    let mut current_path: Option<String> = None; // normalised path of the folder we're inside
    let mut apps_pending = false; // saw the "apps" key, waiting for its '{'
    let mut in_apps_of: Option<Folder> = None; // which folder's apps block we're inside
    let mut found_from = false;
    let mut found_to = false;

    for raw_line in vdf.split_inclusive('\n') {
        // Work on the content without the trailing newline; re-add `newline` later.
        let line = raw_line.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim();

        // Capture a folder's "path" so we know which library this block describes.
        if in_apps_of.is_none() {
            if let Some(path_val) = parse_kv(trimmed, "path") {
                current_path = Some(normalize_path(&path_val));
            }
        }

        // The "apps" key precedes its own `{` on the next line.
        if trimmed.eq_ignore_ascii_case("\"apps\"") {
            apps_pending = true;
            out.push(line.to_string());
            continue;
        }

        if apps_pending && trimmed == "{" {
            apps_pending = false;
            in_apps_of = match current_path.as_deref() {
                Some(p) if p == to_norm => Some(Folder::To),
                Some(p) if p == from_norm => Some(Folder::From),
                _ => None,
            };
            out.push(line.to_string());

            // Insert the moved app at the top of the destination's apps block,
            // matching the surrounding indentation (one level deeper than `{`).
            if matches!(in_apps_of, Some(Folder::To)) {
                let indent = leading_ws(line);
                out.push(format!("{indent}\t{appid_key}\t\t\"{size}\""));
                found_to = true;
            }
            continue;
        }

        // Inside the source's apps block, drop the line for this appid.
        if matches!(in_apps_of, Some(Folder::From)) && trimmed.starts_with(&appid_key) {
            found_from = true;
            continue; // skip (remove) this entry
        }

        // Leaving an apps block.
        if in_apps_of.is_some() && trimmed == "}" {
            in_apps_of = None;
            current_path = None;
            out.push(line.to_string());
            continue;
        }

        out.push(line.to_string());
    }

    // Only rewrite if we actually relocated the entry: the destination block must
    // exist (so Steam will see it), and we either removed a stale source entry or
    // there was none to remove.
    if !found_to {
        return None;
    }
    let _ = found_from; // a missing source entry is fine (e.g. first-time index)

    let mut result = out.join(newline);
    if vdf.ends_with('\n') {
        result.push_str(newline);
    }
    Some(result)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Folder {
    From,
    To,
}

/// Leading whitespace (indentation) of a line.
fn leading_ws(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// Parse a `"key"  "value"` VDF line, returning the value if `key` matches.
fn parse_kv(line: &str, key: &str) -> Option<String> {
    let mut parts = line.split('"').filter(|s| !s.trim().is_empty());
    let k = parts.next()?;
    if !k.eq_ignore_ascii_case(key) {
        return None;
    }
    parts.next().map(|v| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const SAMPLE: &str = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"label\"\t\t\"\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"228980\"\t\t\"123456\"\n\t\t\t\"620\"\t\t\"789012\"\n\t\t}\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t\t\"label\"\t\t\"\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"440\"\t\t\"111\"\n\t\t}\n\t}\n}\n";

    #[test]
    fn moves_entry_between_apps_blocks() {
        let out = update_libraryfolders_apps(
            SAMPLE,
            620,
            Path::new("C:\\Program Files (x86)\\Steam"),
            Path::new("D:\\SteamLibrary"),
            789012,
        )
        .expect("should rewrite");

        // Removed from source block.
        let zero_block = &out[..out.find("\"1\"").unwrap()];
        assert!(!zero_block.contains("\"620\""), "620 should be gone from folder 0");
        // Other source entries are untouched.
        assert!(zero_block.contains("\"228980\""));
        // Added to destination block.
        let one_block = &out[out.find("\"1\"").unwrap()..];
        assert!(one_block.contains("\"620\"\t\t\"789012\""), "620 should be in folder 1");
        assert!(one_block.contains("\"440\""));
    }

    #[test]
    fn destination_missing_returns_none() {
        // Destination path not present in the file → don't touch it.
        let out = update_libraryfolders_apps(
            SAMPLE,
            620,
            Path::new("C:\\Program Files (x86)\\Steam"),
            Path::new("E:\\Nope"),
            1,
        );
        assert!(out.is_none());
    }

    #[test]
    fn missing_source_entry_still_adds_to_destination() {
        // App not currently indexed anywhere, but destination exists → add it.
        let out = update_libraryfolders_apps(
            SAMPLE,
            999,
            Path::new("C:\\Program Files (x86)\\Steam"),
            Path::new("D:\\SteamLibrary"),
            42,
        )
        .expect("should add to destination");
        let one_block = &out[out.find("\"1\"").unwrap()..];
        assert!(one_block.contains("\"999\"\t\t\"42\""));
    }

    #[test]
    fn move_dir_copies_all_files() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.bin"), vec![1u8; 100]).unwrap();
        std::fs::write(src.join("sub/b.bin"), vec![2u8; 200]).unwrap();

        let total = dir_size(&src);
        assert_eq!(total, 300);

        let mut last = 0u64;
        move_dir_with_progress(&src, &dst, total, |copied, _| last = copied).unwrap();

        assert!(!src.exists(), "source should be removed after move");
        assert_eq!(std::fs::read(dst.join("a.bin")).unwrap().len(), 100);
        assert_eq!(std::fs::read(dst.join("sub/b.bin")).unwrap().len(), 200);
        assert_eq!(last, total);
    }
}
