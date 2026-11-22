//! Depot browser: offline manifest helpers.

use anyhow::{Context, Result, anyhow, bail};
use sha1::Digest;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::steam_client::DepotManifestInfo;

/// Steam `EDepotFileFlag::Executable`.
pub const FLAG_EXECUTABLE: u32 = 32;
/// Steam `EDepotFileFlag::Directory`.
pub const FLAG_DIRECTORY: u32 = 64;
/// Steam `EDepotFileFlag::Symlink`.
pub const FLAG_SYMLINK: u32 = 512;

/// What a manifest entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
}

/// One entry of a depot manifest.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ManifestFileEntry {
    /// Path inside the depot, `/`-separated.
    pub filename: String,
    pub kind: EntryKind,
    /// Uncompressed size in bytes.
    pub size: u64,
    /// Content SHA-1 hex; dirs empty.
    pub sha1: String,
    pub chunks: usize,
    pub executable: bool,
    /// Symlink target, when given.
    pub link_target: Option<String>,
}

/// Raw manifest fields of one entry.
#[derive(Debug, Clone, Default)]
pub struct RawEntry {
    pub path: String,
    pub flags: u32,
    pub link_target: String,
    pub size: u64,
    pub sha_content: Vec<u8>,
    pub chunks: usize,
}

/// Kind from flags and target.
pub fn entry_kind(flags: u32, link_target: &str) -> EntryKind {
    if flags & FLAG_DIRECTORY != 0 {
        EntryKind::Directory
    } else if !link_target.is_empty() || flags & FLAG_SYMLINK != 0 {
        EntryKind::Symlink
    } else {
        EntryKind::File
    }
}

impl ManifestFileEntry {
    /// Row for one raw manifest entry.
    pub fn from_raw(raw: RawEntry) -> Self {
        let kind = entry_kind(raw.flags, &raw.link_target);
        Self {
            filename: raw.path,
            kind,
            size: raw.size,
            sha1: if kind == EntryKind::Directory {
                String::new()
            } else {
                hex::encode(&raw.sha_content)
            },
            chunks: raw.chunks,
            executable: raw.flags & FLAG_EXECUTABLE != 0,
            link_target: (!raw.link_target.is_empty()).then(|| raw.link_target.replace('\\', "/")),
        }
    }
}

/// Sorted rows, without depot root.
pub fn entries(raw: impl IntoIterator<Item = RawEntry>) -> Vec<ManifestFileEntry> {
    let mut out: Vec<_> = raw
        .into_iter()
        .map(ManifestFileEntry::from_raw)
        .filter(|e| !e.filename.is_empty())
        .collect();
    out.sort_by(|a, b| a.filename.cmp(&b.filename));
    out
}

/// Total size of the regular files.
pub fn total_size(files: &[ManifestFileEntry]) -> u64 {
    files
        .iter()
        .filter(|f| f.kind == EntryKind::File)
        .map(|f| f.size)
        .sum()
}

/// Case-insensitive `*`/`?` glob match.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let path: Vec<char> = path.to_lowercase().chars().collect();
    let (mut p, mut s) = (0, 0);
    // Last star and its match end.
    let mut star: Option<(usize, usize)> = None;
    while s < path.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == path[s]) {
            p += 1;
            s += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, s));
            p += 1;
        } else if let Some((star_p, star_s)) = star {
            p = star_p + 1;
            s = star_s + 1;
            star = Some((star_p, star_s + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

/// Which manifest of a depot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestRef {
    /// The current manifest on this branch.
    Branch(String),
    /// A specific manifest id.
    Id(u64),
}

impl Default for ManifestRef {
    fn default() -> Self {
        Self::Branch("public".to_string())
    }
}

impl ManifestRef {
    /// Id, else branch, else `public`.
    pub fn from_options(manifest: Option<u64>, branch: Option<&str>) -> Self {
        match (manifest, branch) {
            (Some(id), _) => Self::Id(id),
            (None, Some(branch)) => Self::Branch(branch.to_string()),
            (None, None) => Self::default(),
        }
    }
}

/// Manifest id for `reference`.
pub fn pick_manifest(
    manifests: &[DepotManifestInfo],
    depot_id: u32,
    reference: &ManifestRef,
) -> Result<u64> {
    let branch = match reference {
        ManifestRef::Id(id) => return Ok(*id),
        ManifestRef::Branch(branch) => branch,
    };
    let depot: Vec<_> = manifests
        .iter()
        .filter(|m| m.depot_id == depot_id)
        .collect();
    if depot.is_empty() {
        bail!(
            "depot {depot_id} has no manifests in this app (check `aurelia depots` for its depots)"
        );
    }
    depot
        .iter()
        .find(|m| m.branch.eq_ignore_ascii_case(branch))
        .map(|m| m.manifest_id)
        .ok_or_else(|| {
            let branches: Vec<_> = depot.iter().map(|m| m.branch.as_str()).collect();
            anyhow!(
                "depot {depot_id} has no manifest on branch {branch:?} (branches: {})",
                branches.join(", ")
            )
        })
}

/// One depot file to download.
#[derive(Debug, Clone)]
pub struct DepotFileDownload {
    pub app_id: u32,
    pub depot_id: u32,
    pub manifest: ManifestRef,
    /// Path inside the depot.
    pub path: String,
    /// Created if missing.
    pub out_dir: PathBuf,
    /// Replace an existing file.
    pub force: bool,
}

/// A finished single-file download.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadedFile {
    pub manifest_id: u64,
    pub path: PathBuf,
    pub size: u64,
}

/// Request path in manifest form.
pub fn normalize_request(path: &str) -> String {
    let path = path.trim().replace('\\', "/");
    let mut path = path.as_str();
    loop {
        if let Some(rest) = path.strip_prefix("./") {
            path = rest;
        } else if let Some(rest) = path.strip_prefix('/') {
            path = rest;
        } else {
            break;
        }
    }
    path.to_string()
}

/// Only regular files can be downloaded.
pub fn ensure_file(path: &str, kind: EntryKind) -> Result<()> {
    match kind {
        EntryKind::File => Ok(()),
        EntryKind::Directory => bail!("{path} is a directory; download its files one by one"),
        EntryKind::Symlink => bail!("{path} is a symlink, not a file"),
    }
}

/// Confined `out_dir/<file name>` target.
pub fn download_target(out_dir: &Path, depot_path: &str) -> Result<PathBuf> {
    let relative = crate::core::paths::safe_relative_path(&normalize_request(depot_path))
        .ok_or_else(|| anyhow!("unsafe depot path: {depot_path:?}"))?;
    let name = relative
        .file_name()
        .ok_or_else(|| anyhow!("{depot_path:?} has no file name"))?;
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("failed creating {}", out_dir.display()))?;
    let root = out_dir
        .canonicalize()
        .with_context(|| format!("failed resolving {}", out_dir.display()))?;
    let dest = root.join(name);
    if !crate::core::paths::resolves_within(&root, &dest) {
        bail!("{} leaves {}", dest.display(), root.display());
    }
    Ok(dest)
}

/// Partial file next to `dest`.
pub fn part_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    dest.with_file_name(format!("{name}.part"))
}

/// Hex SHA-1 of a file.
pub fn sha1_file(path: &Path) -> Result<String> {
    let mut reader =
        std::fs::File::open(path).with_context(|| format!("failed opening {}", path.display()))?;
    let mut hasher = sha1::Sha1::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Refuse an existing `dest` unless `force`.
pub fn check_dest(dest: &Path, force: bool) -> Result<()> {
    if dest.is_dir() {
        bail!("{} is a directory", dest.display());
    }
    if !force && dest.symlink_metadata().is_ok() {
        bail!(
            "{} already exists (use --force to overwrite)",
            dest.display()
        );
    }
    Ok(())
}

/// Fresh empty partial for `dest`.
pub fn create_part(dest: &Path) -> Result<PathBuf> {
    let part = part_path(dest);
    match std::fs::remove_file(&part) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("failed removing {}", part.display())),
    }
    // Never follows a planted link.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .with_context(|| format!("failed creating {}", part.display()))?;
    Ok(part)
}

/// What the finished `.part` must match.
#[derive(Debug, Clone, Copy)]
pub struct PlaceCheck<'a> {
    /// Manifest SHA-1 hex; empty skips.
    pub sha1: &'a str,
    pub size: u64,
    pub force: bool,
}

/// Downloaded content fails its SHA-1.
#[derive(Debug)]
pub struct ChecksumMismatch {
    pub path: PathBuf,
    pub expected: String,
    pub actual: String,
}

impl std::fmt::Display for ChecksumMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} failed its checksum (expected SHA-1 {}, got {})",
            self.path.display(),
            self.expected,
            self.actual
        )
    }
}

impl std::error::Error for ChecksumMismatch {}

/// Verify, then move; failure removes `part`.
pub fn place(part: &Path, dest: &Path, check: PlaceCheck<'_>) -> Result<()> {
    let result = verify_and_move(part, dest, check);
    if result.is_err() {
        let _ = std::fs::remove_file(part);
    }
    result
}

fn verify_and_move(part: &Path, dest: &Path, check: PlaceCheck<'_>) -> Result<()> {
    if !check.sha1.is_empty() && check.size > 0 {
        let actual = sha1_file(part)?;
        if !actual.eq_ignore_ascii_case(check.sha1) {
            return Err(ChecksumMismatch {
                path: dest.to_path_buf(),
                expected: check.sha1.to_string(),
                actual,
            }
            .into());
        }
    }
    if check.force {
        return std::fs::rename(part, dest)
            .with_context(|| format!("failed moving the download to {}", dest.display()));
    }
    // Hard link fails if dest appeared.
    match std::fs::hard_link(part, dest) {
        Ok(()) => {
            let _ = std::fs::remove_file(part);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => bail!(
            "{} already exists (use --force to overwrite)",
            dest.display()
        ),
        // No hard links here; recheck.
        Err(_) => {
            check_dest(dest, false)?;
            std::fs::rename(part, dest)
                .with_context(|| format!("failed moving the download to {}", dest.display()))
        }
    }
}

#[cfg(test)]
mod tests;
