use futures::{stream::FuturesOrdered, StreamExt};
use itertools::Itertools;
use sha1::Digest;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing;
use std::{fmt::Write, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::Semaphore,
};

use crate::{cdn::inner::InnerClient, Error};

/// Steam EDepotFileFlag::Executable bit.
pub const FLAG_EXECUTABLE: u32 = 32;

/// Steam EDepotFileFlag::Directory bit.
pub const FLAG_DIRECTORY: u32 = 64;

/// Steam EDepotFileFlag::Symlink bit.
pub const FLAG_SYMLINK: u32 = 512;

#[derive(Debug)]
pub struct ChunkData {
    pub(super) sha: Vec<u8>,
    pub(super) crc: u32,
    pub(super) offset: u64,
    pub(super) original_size: u32,
    pub(super) compressed_size: u32,
}

impl ChunkData {
    pub fn sha(&self) -> Vec<u8> {
        self.sha.clone()
    }

    pub fn id(&self) -> String {
        self.sha.iter().fold(String::new(), |mut output, b| {
            let _ = write!(output, "{b:02x}");
            output
        })
    }

    pub fn crc(&self) -> u32 {
        self.crc
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn original_size(&self) -> u32 {
        self.original_size
    }

    pub fn compressed_size(&self) -> u32 {
        self.compressed_size
    }
}

#[derive(Debug)]
pub struct ManifestFile {
    pub(super) inner: Arc<InnerClient>,
    pub(super) app_id: u32,
    pub(super) depot_id: u32,
    pub(super) filename: String,
    pub(super) size: u64,
    pub(super) flags: u32,
    pub(super) sha_filename: Vec<u8>,
    pub(super) sha_content: Vec<u8>,
    pub(super) chunks: Vec<ChunkData>,
    pub(super) linktarget: String,
}

impl ManifestFile {
    pub fn full_path(&self) -> String {
        self.filename
            .trim_matches(|c: char| c.is_whitespace() || c == '\0')
            .replace("\\", "/")
    }

    pub fn path(&self) -> String {
        let full_path = self.full_path();
        if let Some(pos) = full_path.rfind('/') {
            full_path[..pos].to_string()
        } else {
            full_path
        }
    }

    pub fn filename(&self) -> String {
        self.full_path()
            .rsplit("/")
            .next()
            .unwrap_or("")
            .to_string()
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn flags(&self) -> u32 {
        self.flags
    }

    pub fn sha_filename(&self) -> Vec<u8> {
        self.sha_filename.clone()
    }

    pub fn sha_content(&self) -> Vec<u8> {
        self.sha_content.clone()
    }

    pub fn chunks(&self) -> &Vec<ChunkData> {
        self.chunks.as_ref()
    }

    pub fn linktarget(&self) -> String {
        self.linktarget.clone()
    }

    /// Fetch this file's full content into memory. Symlink entries store
    /// their link target as chunk data, so they need content without a
    /// destination file on disk.
    pub async fn download_to_memory(&self, depot_key: [u8; 32]) -> Result<Vec<u8>, Error> {
        let mut buffer = vec![0u8; self.size as usize];
        for chunk in self.chunks().iter().sorted_by(|a, b| a.offset.cmp(&b.offset)) {
            let data = self
                .inner
                .get_chunk(self.app_id, self.depot_id, depot_key, chunk.id())
                .await?;
            let offset = chunk.offset as usize;
            let end = offset.saturating_add(data.len()).min(buffer.len());
            if offset < end {
                buffer[offset..end].copy_from_slice(&data[..end - offset]);
            }
        }
        Ok(buffer)
    }

    /// Download into `target_path`, reusing matching chunks.
    ///
    /// `on_progress(bytes, reused)` fires per chunk.
    pub async fn download(
        &self,
        depot_key: [u8; 32],
        target_path: &std::path::Path,
        verify_mode: bool,
        abort_signal: Option<Arc<AtomicBool>>,
        limit: Option<Arc<Semaphore>>,
        on_progress: Option<ProgressFn>,
    ) -> Result<(), Error> {
        let _ = verify_mode;
        let semaphore = limit.unwrap_or_else(|| Arc::new(Semaphore::new(4)));

        let metadata_before = tokio::fs::metadata(target_path).await;
        let file_exists_before = metadata_before.is_ok();
        let current_len_before = metadata_before.map(|m| m.len()).unwrap_or(0);

        let mut out = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(target_path)
            .await
            .map_err(Error::Io)?;

        if current_len_before != self.size {
            out.set_len(self.size).await.map_err(Error::Io)?;
        }

        let mut tasks = self
            .chunks()
            .iter()
            .sorted_by(|&a, &b| a.offset.cmp(&b.offset))
            .map(|chunk_data| {
                let job = ChunkJob {
                    inner: self.inner.clone(),
                    app_id: self.app_id,
                    depot_id: self.depot_id,
                    depot_key,
                    id: chunk_data.id(),
                    sha: chunk_data.sha.clone(),
                    offset: chunk_data.offset,
                    size: chunk_data.original_size,
                    target_path: target_path.to_path_buf(),
                    // Only existing files hold chunks.
                    check_disk: file_exists_before,
                };
                AbortOnDrop(tokio::spawn(job.run(
                    semaphore.clone(),
                    abort_signal.clone(),
                    on_progress.clone(),
                )))
            })
            .collect::<FuturesOrdered<_>>();

        while let Some(result) = tasks.next().await {
            if let Some(signal) = &abort_signal {
                if signal.load(Ordering::Relaxed) {
                    break;
                }
            }
            let (offset, data) = result??;
            if let Some(data) = data {
                let len = data.len() as u64;
                out.seek(tokio::io::SeekFrom::Start(offset))
                    .await
                    .map_err(Error::Io)?;
                out.write_all(&data).await.map_err(Error::Io)?;
                if let Some(ref cb) = on_progress {
                    cb(len, false);
                }
            }
        }
        out.flush().await.map_err(Error::Io)?;

        #[cfg(unix)]
        self.apply_unix_mode(target_path).await?;

        Ok(())
    }

    #[cfg(unix)]
    pub(crate) async fn apply_unix_mode(&self, target_path: &std::path::Path) -> Result<(), Error> {
        use std::os::unix::fs::PermissionsExt;

        if self.flags & FLAG_EXECUTABLE == 0 {
            // Flagless depots: detect ELF/shebang content.
            let mut head = [0u8; 4];
            let looks_executable = match tokio::fs::File::open(target_path).await {
                Ok(mut f) => match f.read(&mut head).await {
                    Ok(n) => (n >= 4 && head == *b"\x7fELF") || (n >= 2 && head[..2] == *b"#!"),
                    Err(_) => false,
                },
                Err(_) => false,
            };
            if !looks_executable {
                return Ok(());
            }
        }
        let metadata = tokio::fs::metadata(target_path)
            .await
            .map_err(Error::Io)?;
        let mut permissions = metadata.permissions();
        if permissions.mode() & 0o111 != 0 {
            return Ok(());
        }
        permissions.set_mode(permissions.mode() | 0o755);
        tokio::fs::set_permissions(target_path, permissions)
            .await
            .map_err(Error::Io)?;
        Ok(())
    }
}

/// `(bytes, reused_from_disk)` progress callback.
pub type ProgressFn = Arc<dyn Fn(u64, bool) + Send + Sync + 'static>;

/// Spawned task, aborted when dropped.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl<T> std::future::Future for AbortOnDrop<T> {
    type Output = Result<T, tokio::task::JoinError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.0).poll(cx)
    }
}

/// One chunk: reuse or fetch.
struct ChunkJob {
    inner: Arc<InnerClient>,
    app_id: u32,
    depot_id: u32,
    depot_key: [u8; 32],
    id: String,
    sha: Vec<u8>,
    offset: u64,
    size: u32,
    target_path: std::path::PathBuf,
    check_disk: bool,
}

impl ChunkJob {
    /// `(offset, None)` when reused or aborted.
    async fn run(
        self,
        semaphore: Arc<Semaphore>,
        abort_signal: Option<Arc<AtomicBool>>,
        on_progress: Option<ProgressFn>,
    ) -> Result<(u64, Option<Vec<u8>>), Error> {
        let aborted = || abort_signal.as_ref().is_some_and(|s| s.load(Ordering::Relaxed));
        if aborted() {
            return Ok((self.offset, None));
        }
        let _permit = semaphore.acquire_owned().await?;
        if aborted() {
            return Ok((self.offset, None));
        }
        if self.check_disk && self.on_disk().await? {
            tracing::debug!("Verified chunk {}", self.id);
            if let Some(cb) = &on_progress {
                cb(self.size as u64, true);
            }
            return Ok((self.offset, None));
        }
        let data = self
            .inner
            .get_chunk(self.app_id, self.depot_id, self.depot_key, self.id.clone())
            .await?;
        Ok((self.offset, Some(data)))
    }

    /// Whether the bytes on disk match.
    async fn on_disk(&self) -> Result<bool, Error> {
        let Ok(mut file) = tokio::fs::File::open(&self.target_path).await else {
            return Ok(false);
        };
        let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
        if len < self.offset + self.size as u64 {
            return Ok(false);
        }
        file.seek(tokio::io::SeekFrom::Start(self.offset))
            .await
            .map_err(Error::Io)?;
        let mut buffer = vec![0u8; self.size as usize];
        if file.read_exact(&mut buffer).await.is_err() {
            return Ok(false);
        }
        let matches = sha1::Sha1::digest(&buffer).as_slice() == self.sha.as_slice();
        if !matches {
            tracing::warn!("Corruption detected in chunk {}", self.id);
        }
        Ok(matches)
    }
}
