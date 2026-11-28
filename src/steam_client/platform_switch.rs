//! Reuse the installed build when an install switches platform.
//!
//! Like Steam: files whose content didn't change are linked to their new
//! paths, and changed files are rebuilt from any old chunk with the same
//! hash, so only chunks found nowhere on disk are downloaded. The old build
//! stays whole until the switch finishes, so an interrupted one leaves a
//! valid install behind.
use super::depot_files::ListedFile;
use super::*;
use crate::library::depot_browser::{EntryKind, entry_kind};
use sha1::Digest;
use std::io::{Read, Seek, SeekFrom, Write};

/// `(source app, depot, manifest)`.
type Depot = (u32, u32, u64);

/// An install replacing a build of another platform.
pub(crate) struct PlatformSwitch {
    /// Installed depots of the other platform.
    stale: Vec<Depot>,
    /// Other installed depots the new build doesn't select; they stay.
    kept: Vec<Depot>,
    files: Option<SwitchFiles>,
}

/// Both builds' manifests.
#[derive(Default)]
struct SwitchFiles {
    old: Vec<ListedFile>,
    new: Vec<ListedFile>,
    kept: Vec<ListedFile>,
    /// New or kept depots whose manifests couldn't be read.
    unknown: HashSet<u32>,
}

impl PlatformSwitch {
    /// The switch an install over `installed` makes, if any of those depots
    /// belong to another platform.
    pub(crate) fn plan(
        installed: &HashMap<u64, u64>,
        selections: &[ManifestSelection],
        resolved: &depots::ResolvedDepots,
        platform: DepotPlatform,
    ) -> Option<Self> {
        let (stale, kept): (Vec<Depot>, Vec<Depot>) = installed
            .iter()
            .map(|(&depot, &manifest)| (resolved.source_app(depot as u32), depot as u32, manifest))
            .filter(|&(_, depot, _)| !selections.iter().any(|s| s.depot_id == depot))
            .partition(|&(_, depot, _)| {
                resolved
                    .rows
                    .get(&depot.to_string())
                    .is_some_and(|row| !should_keep_depot(depots::depot_oslist(row), platform))
            });
        (!stale.is_empty()).then_some(Self {
            stale,
            kept,
            files: None,
        })
    }

    /// Old depots to keep listed while switching, so a resumed switch knows them.
    pub(crate) fn pending_depots(&self) -> Vec<(u32, u64, u64)> {
        self.stale.iter().map(|&(_, d, m)| (d, m, 0)).collect()
    }

    /// Put the old build's content where the new one expects it. Returns the
    /// new build's size and the bytes of it still to fetch.
    pub(crate) async fn prepare(
        &mut self,
        client: &SteamClient,
        root: &Path,
        installed: &HashMap<u64, u64>,
        selections: &[ManifestSelection],
    ) -> Option<(u64, u64)> {
        let new: Vec<Depot> = selections
            .iter()
            .map(|s| (s.app_id, s.depot_id, s.manifest_id))
            .collect();
        // Selected depots already installed belong to the old build too.
        let old: Vec<Depot> = selections
            .iter()
            .filter_map(|s| {
                Some((
                    s.app_id,
                    s.depot_id,
                    *installed.get(&u64::from(s.depot_id))?,
                ))
            })
            .chain(self.stale.iter().copied())
            .collect();
        let files = client.switch_files(&old, &new, &self.kept).await;
        let root = root.to_path_buf();
        let (files, missing) = tokio::task::spawn_blocking(move || {
            let missing = reuse_old_build(&root, &files.old, &files.new);
            (files, missing)
        })
        .await
        .ok()?;
        let total = files
            .new
            .iter()
            .filter(|f| regular(f))
            .map(|f| f.raw.size)
            .sum();
        self.files = Some(files);
        Some((total, missing))
    }

    /// Delete what only the old build used and unlist its depots.
    pub(crate) fn finish(
        &self,
        root: &Path,
        manifest_path: &Path,
        installed: &[(u32, u64, u64)],
    ) -> Result<()> {
        if let Some(files) = &self.files {
            let unread = files.unknown.iter().any(|d| {
                installed.iter().any(|(id, ..)| id == d)
                    || self.kept.iter().any(|&(_, id, _)| id == *d)
            });
            if !unread {
                let paths = |lists: &[&Vec<ListedFile>]| {
                    vec![
                        lists
                            .iter()
                            .flat_map(|l| l.iter().map(|f| f.raw.path.clone()))
                            .collect(),
                    ]
                };
                let doomed = super::language::files_to_delete(
                    &paths(&[&files.old]),
                    &paths(&[&files.new, &files.kept]),
                );
                super::language::delete_files(root, &doomed);
            }
        }
        let dropped = self.stale.iter().map(|&(_, d, _)| d).collect();
        let raw = std::fs::read_to_string(manifest_path)?;
        let updated =
            super::manifests::replace_depots_in_manifest(&raw, &dropped, &[], &HashMap::new());
        crate::core::atomic::write_atomic(manifest_path, updated.as_bytes())
    }
}

/// Where a known chunk sits on disk.
type ChunkSource<'a> = (&'a Path, u64);

fn regular(f: &ListedFile) -> bool {
    entry_kind(f.raw.flags, &f.raw.link_target) == EntryKind::File
}

fn key(path: &str) -> String {
    path.to_ascii_lowercase()
}

/// Put the old build's content where the new build expects it. Returns the
/// new-build bytes found nowhere on disk.
pub(crate) fn reuse_old_build(root: &Path, old: &[ListedFile], new: &[ListedFile]) -> u64 {
    let new_paths: HashSet<String> = new.iter().map(|f| key(&f.raw.path)).collect();
    let mut old_at = HashMap::new();
    let mut linkable = HashMap::new();
    let mut chunks: HashMap<&[u8], ChunkSource<'_>> = HashMap::new();
    let present: Vec<(&ListedFile, PathBuf)> = old
        .iter()
        .filter(|f| regular(f))
        .inspect(|f| {
            old_at.insert(key(&f.raw.path), f.raw.sha_content.as_slice());
        })
        .filter_map(|f| {
            let path = super::language::confined_path(root, &f.raw.path)?;
            let size = std::fs::metadata(&path).ok()?.len();
            (size == f.raw.size).then_some((f, path))
        })
        .collect();
    for (f, path) in &present {
        // A path the new build rewrites can't share its file.
        if !new_paths.contains(&key(&f.raw.path)) {
            linkable.entry(f.raw.sha_content.as_slice()).or_insert(path);
        }
        for (sha, offset, _) in &f.chunks {
            chunks.entry(sha.as_slice()).or_insert((path, *offset));
        }
    }

    // Read everything before placing anything, so every source stays put.
    let mut missing = 0;
    let mut links = Vec::new();
    let mut renames = Vec::new();
    for f in new.iter().filter(|f| regular(f) && f.raw.size > 0) {
        let content = f.raw.sha_content.as_slice();
        if old_at.get(&key(&f.raw.path)) == Some(&content) {
            continue;
        }
        let Some(to) = super::language::confined_path(root, &f.raw.path) else {
            missing += f.raw.size;
            continue;
        };
        if let Some(from) = linkable.get(content) {
            links.push((from.to_path_buf(), to));
            continue;
        }
        let at_target = std::fs::metadata(&to).is_ok_and(|m| m.len() == f.raw.size);
        // Placed by an earlier, interrupted switch.
        if at_target && old_at.values().any(|&sha| sha == content) {
            continue;
        }
        let found = if f
            .chunks
            .iter()
            .any(|(sha, ..)| chunks.contains_key(sha.as_slice()))
        {
            let part = part_path(&to);
            match rebuild(&part, &to, f, &chunks) {
                Ok(0) => 0,
                Ok(found) => {
                    renames.push((part, to));
                    found
                }
                Err(e) => {
                    tracing::warn!("could not rebuild {} from the old build: {e}", f.raw.path);
                    let _ = std::fs::remove_file(&part);
                    0
                }
            }
        } else if at_target {
            held_bytes(&to, f)
        } else {
            0
        };
        missing += f.raw.size - found;
    }
    tracing::info!(
        "Reusing the installed build: {} file(s) linked, {} rebuilt, {missing} byte(s) to fetch",
        links.len(),
        renames.len()
    );
    for (from, to) in &links {
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::remove_file(to);
        if std::fs::hard_link(from, to).is_err() {
            let _ = std::fs::rename(from, to);
        }
    }
    for (part, to) in renames {
        let _ = std::fs::rename(&part, &to);
    }
    missing
}

fn part_path(to: &Path) -> PathBuf {
    let mut name = to.file_name().unwrap_or_default().to_os_string();
    name.push(".aurelia-reuse");
    to.with_file_name(name)
}

/// Write `f` to `part` from known chunks, keeping chunks `to` already holds.
/// Returns the bytes found; the rest stays zeroed for the download to fill.
/// Nothing is left behind when nothing was found.
fn rebuild(
    part: &Path,
    to: &Path,
    f: &ListedFile,
    chunks: &HashMap<&[u8], ChunkSource<'_>>,
) -> std::io::Result<u64> {
    let _ = std::fs::remove_file(part);
    let mut open = HashMap::new();
    let mut out = None;
    let mut found = 0;
    for (sha, offset, size) in &f.chunks {
        let known = chunks.get(sha.as_slice()).copied();
        let Some(buf) = known
            .into_iter()
            .chain([(to, *offset)])
            .find_map(|(source, at)| read_chunk(&mut open, source, at, sha, *size))
        else {
            continue;
        };
        let out = match &mut out {
            Some(out) => out,
            None => {
                if let Some(parent) = part.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let file = std::fs::File::create(part)?;
                file.set_len(f.raw.size)?;
                out.insert(file)
            }
        };
        out.seek(SeekFrom::Start(*offset))?;
        out.write_all(&buf)?;
        found += u64::from(*size);
    }
    Ok(found)
}

/// Bytes of `f`'s chunks `path` already holds in place.
fn held_bytes(path: &Path, f: &ListedFile) -> u64 {
    let mut open = HashMap::new();
    f.chunks
        .iter()
        .filter(|(sha, offset, size)| read_chunk(&mut open, path, *offset, sha, *size).is_some())
        .map(|(_, _, size)| u64::from(*size))
        .sum()
}

/// The chunk at `at` in `source`, if its hash is `sha`.
fn read_chunk(
    open: &mut HashMap<PathBuf, std::fs::File>,
    source: &Path,
    at: u64,
    sha: &[u8],
    size: u32,
) -> Option<Vec<u8>> {
    let file = match open.entry(source.to_path_buf()) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(e) => e.insert(std::fs::File::open(source).ok()?),
    };
    let mut buf = vec![0; size as usize];
    file.seek(SeekFrom::Start(at)).ok()?;
    file.read_exact(&mut buf).ok()?;
    (sha1::Sha1::digest(&buf).as_slice() == sha).then_some(buf)
}

impl SteamClient {
    /// Both builds' manifests, read in parallel. One that can't be read (an
    /// unowned DLC depot) is left out.
    async fn switch_files(&self, old: &[Depot], new: &[Depot], kept: &[Depot]) -> SwitchFiles {
        let limit = Arc::new(tokio::sync::Semaphore::new(16));
        let mut tasks = tokio::task::JoinSet::new();
        let jobs = [(0, old), (1, new), (2, kept)]
            .into_iter()
            .flat_map(|(side, depots)| depots.iter().map(move |&depot| (side, depot)));
        for (side, (app, depot, manifest)) in jobs {
            let client = self.clone();
            let limit = Arc::clone(&limit);
            tasks.spawn(async move {
                let _permit = limit.acquire_owned().await;
                (
                    side,
                    depot,
                    client.fetch_manifest_listing(app, depot, manifest).await,
                )
            });
        }
        let mut files = SwitchFiles::default();
        while let Some(Ok((side, depot, listing))) = tasks.join_next().await {
            match (side, listing) {
                (0, Ok(list)) => files.old.extend(list),
                (1, Ok(list)) => files.new.extend(list),
                (_, Ok(list)) => files.kept.extend(list),
                (0, Err(e)) => tracing::warn!("manifest of old depot {depot}: {e}"),
                (_, Err(_)) => {
                    files.unknown.insert(depot);
                }
            }
        }
        files
    }
}
