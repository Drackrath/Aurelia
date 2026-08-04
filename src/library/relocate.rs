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

/// Minimal well-formed `libraryfolders.vdf` used to rebuild a file whose root
/// block cannot be located (missing or malformed beyond line-level repair).
const EMPTY_LIBRARYFOLDERS: &str = "\"libraryfolders\"\n{\n}\n";

/// The Wine view of a native (host) absolute path: Wine maps the whole host
/// filesystem at drive `Z:` by default, so `/home/x/SteamLibrary` becomes
/// `Z:\home\x\SteamLibrary` — the only form the in-Wine Steam client can open.
/// Paths already carrying a drive letter are only separator-normalised.
pub fn to_wine_path(native: &Path) -> String {
    let s = native.to_string_lossy().replace('/', "\\");
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        s
    } else {
        format!("Z:{s}")
    }
}

/// Escape a path for use as a VDF string value (backslashes are doubled).
fn vdf_escape(path: &str) -> String {
    path.replace('\\', "\\\\")
}

/// Whether `libraryfolders.vdf` text already registers `lib_path` as a library
/// folder (compared with the same normalisation the editors use).
pub fn libraryfolders_registers_path(vdf: &str, lib_path: &str) -> bool {
    let target = normalize_path(lib_path);
    vdf.lines().any(|line| {
        parse_kv(line.trim(), "path").is_some_and(|p| normalize_path(&p) == target)
    })
}

/// Register (or refresh) a library-folder entry for `lib_path` in
/// `libraryfolders.vdf` text, carrying an `apps` index of `(appid, size)` pairs.
///
/// If an entry with the same (normalised) path already exists it is replaced in
/// place; otherwise a new entry is appended under the next free numeric key.
/// Every other line is copied through unchanged. When the file has no
/// recognisable root block at all (missing/corrupt), the entry is written into a
/// fresh minimal file instead of appending to garbage the client would reject
/// wholesale.
pub fn upsert_libraryfolders_library(vdf: &str, lib_path: &str, apps: &[(u32, u64)]) -> String {
    match try_upsert_library(vdf, lib_path, apps) {
        Some(out) => out,
        None => try_upsert_library(EMPTY_LIBRARYFOLDERS, lib_path, apps)
            .expect("the empty libraryfolders template is well-formed"),
    }
}

/// Core of [`upsert_libraryfolders_library`]; `None` when no root block closing
/// brace can be found (the caller then rebuilds from the empty template).
fn try_upsert_library(vdf: &str, lib_path: &str, apps: &[(u32, u64)]) -> Option<String> {
    let newline = if vdf.contains("\r\n") { "\r\n" } else { "\n" };
    let target = normalize_path(lib_path);
    let lines: Vec<&str> = vdf.lines().collect();

    let mut depth: usize = 0;
    // (start line, key, normalised path) of the top-level entry being scanned.
    let mut cur: Option<(usize, String, Option<String>)> = None;
    // (start line, end line, key) of the entry matching `lib_path`, if any.
    let mut replace: Option<(usize, usize, String)> = None;
    let mut next_key: u32 = 0;
    let mut root_close: Option<usize> = None;

    for (idx, raw) in lines.iter().enumerate() {
        let trimmed = raw.trim();
        if depth == 1 && cur.is_none() {
            if let Some(key) = lone_quoted_key(trimmed) {
                if let Ok(n) = key.parse::<u32>() {
                    next_key = next_key.max(n + 1);
                }
                cur = Some((idx, key, None));
                continue;
            }
        }
        if let Some((_, _, path_slot)) = cur.as_mut() {
            if path_slot.is_none() {
                if let Some(v) = parse_kv(trimmed, "path") {
                    *path_slot = Some(normalize_path(&v));
                }
            }
        }
        if trimmed == "{" {
            depth += 1;
        } else if trimmed == "}" {
            depth = depth.saturating_sub(1);
            if depth == 1 {
                if let Some((start, key, path)) = cur.take() {
                    if replace.is_none() && path.as_deref() == Some(target.as_str()) {
                        replace = Some((start, idx, key));
                    }
                }
            }
            if depth == 0 && root_close.is_none() {
                root_close = Some(idx);
            }
        }
    }

    let key = replace
        .as_ref()
        .map(|(_, _, k)| k.clone())
        .unwrap_or_else(|| next_key.to_string());
    let rendered = render_library_entry(&key, lib_path, apps, newline);

    let mut out: Vec<String> = Vec::new();
    if let Some((start, end, _)) = replace {
        for (idx, raw) in lines.iter().enumerate() {
            if idx == start {
                out.push(rendered.clone());
            }
            if (start..=end).contains(&idx) {
                continue;
            }
            out.push((*raw).to_string());
        }
    } else {
        let close = root_close?;
        for (idx, raw) in lines.iter().enumerate() {
            if idx == close {
                out.push(rendered.clone());
            }
            out.push((*raw).to_string());
        }
    }

    let mut result = out.join(newline);
    if vdf.ends_with('\n') || vdf.is_empty() {
        result.push_str(newline);
    }
    Some(result)
}

/// Render one library-folder entry block (Steam's own layout and indentation).
fn render_library_entry(key: &str, lib_path: &str, apps: &[(u32, u64)], newline: &str) -> String {
    let escaped = vdf_escape(lib_path);
    let mut s = String::new();
    s.push_str(&format!("\t\"{key}\"{newline}"));
    s.push_str(&format!("\t{{{newline}"));
    s.push_str(&format!("\t\t\"path\"\t\t\"{escaped}\"{newline}"));
    s.push_str(&format!("\t\t\"label\"\t\t\"\"{newline}"));
    s.push_str(&format!("\t\t\"apps\"{newline}"));
    s.push_str(&format!("\t\t{{{newline}"));
    for (appid, size) in apps {
        s.push_str(&format!("\t\t\t\"{appid}\"\t\t\"{size}\"{newline}"));
    }
    s.push_str(&format!("\t\t}}{newline}"));
    s.push_str("\t}");
    s
}

/// A line consisting of a single quoted token (a VDF block key), e.g. `"0"`.
fn lone_quoted_key(line: &str) -> Option<String> {
    if !line.starts_with('"') {
        return None;
    }
    let mut parts = line.split('"').filter(|s| !s.trim().is_empty());
    let key = parts.next()?.to_string();
    parts.next().is_none().then_some(key)
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
#[path = "relocate_tests.rs"]
mod tests;
