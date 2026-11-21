//! HTTP transfers, retries and body limits.

use super::*;

/// Cloud RPC response timeout.
pub(super) const CLOUD_RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// Cloud HTTP connect timeout.
pub(super) const CLOUD_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Cloud HTTP read (idle) timeout.
pub(super) const CLOUD_READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Waits between transfer attempts.
pub(super) const TRANSFER_BACKOFF: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
];

/// Upload deadline floor rate, bytes/second.
pub(super) const UPLOAD_FLOOR_RATE: u64 = 32 * 1024;

/// Minimum per-block upload deadline.
pub(super) const UPLOAD_MIN_DEADLINE: Duration = Duration::from_secs(60);

/// HTTP client with cloud timeouts.
pub(super) fn cloud_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("aurelia")
        .connect_timeout(CLOUD_CONNECT_TIMEOUT)
        .read_timeout(CLOUD_READ_TIMEOUT)
        .build()
        .unwrap_or_else(|e| {
            tracing::warn!("cloud HTTP client fallback: {e}");
            reqwest::Client::new()
        })
}

/// How an HTTP transfer failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TransferFailure {
    Connect,
    Timeout,
    Body,
    Status(u16),
    Other,
}

/// Classify first HTTP error in chain.
pub(super) fn transfer_failure(err: &anyhow::Error) -> TransferFailure {
    for cause in err.chain() {
        if let Some(e) = cause.downcast_ref::<reqwest::Error>() {
            return if e.is_timeout() {
                TransferFailure::Timeout
            } else if e.is_connect() {
                TransferFailure::Connect
            } else if let Some(status) = e.status() {
                TransferFailure::Status(status.as_u16())
            } else if e.is_body() || e.is_decode() {
                TransferFailure::Body
            } else {
                TransferFailure::Other
            };
        }
    }
    TransferFailure::Other
}

/// Worth retrying: network, timeout, 5xx/429.
pub(super) fn is_transient(failure: TransferFailure) -> bool {
    match failure {
        TransferFailure::Connect | TransferFailure::Timeout | TransferFailure::Body => true,
        TransferFailure::Status(code) => code == 429 || (500..600).contains(&code),
        TransferFailure::Other => false,
    }
}

/// Drop the (signed) URL from errors.
pub(super) fn http_error(e: reqwest::Error) -> anyhow::Error {
    anyhow::Error::new(e.without_url())
}

/// Per-block deadline scaled by size.
pub(super) fn upload_deadline(len: usize) -> Duration {
    let secs = (len as u64).div_ceil(UPLOAD_FLOOR_RATE);
    Duration::from_secs(secs).max(UPLOAD_MIN_DEADLINE)
}

/// Run `op`; retry transient failures.
pub(super) async fn retry_transfer<T, F, Fut>(
    what: &str,
    delays: &[Duration],
    mut op: F,
) -> Result<T>
where
    F: FnMut(usize) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut attempt = 0usize;
    loop {
        attempt += 1;
        match op(attempt).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                let Some(delay) = delays.get(attempt - 1) else {
                    return Err(e);
                };
                if !is_transient(transfer_failure(&e)) {
                    return Err(e);
                }
                tracing::warn!("{what}: attempt {attempt} failed ({e:#}); retrying in {delay:?}");
                tokio::time::sleep(*delay).await;
            }
        }
    }
}

/// Largest accepted unzipped cloud file.
pub(super) const MAX_CLOUD_UNZIPPED: u64 = 1024 * 1024 * 1024;

/// Cap on Steam-supplied capacity hints.
pub(super) const MAX_CLOUD_PREALLOC: u32 = 64 * 1024 * 1024;

/// Slack over the announced transfer size.
pub(super) const CLOUD_BODY_SLACK: u64 = 64 * 1024;

/// Byte limit for one cloud download.
pub(super) fn cloud_body_limit(stored_size: u32, raw_file_size: u32) -> u64 {
    let announced = if stored_size > 0 {
        stored_size
    } else {
        raw_file_size
    };
    if announced == 0 {
        return MAX_CLOUD_UNZIPPED;
    }
    (u64::from(announced) + CLOUD_BODY_SLACK).min(MAX_CLOUD_UNZIPPED)
}

/// Read a response body, refusing oversize.
pub(super) async fn read_body_capped(mut resp: reqwest::Response, limit: u64) -> Result<Vec<u8>> {
    if resp.content_length().is_some_and(|len| len > limit) {
        bail!("cloud download is larger than the {limit}-byte limit");
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(http_error)? {
        push_capped(&mut body, &chunk, limit)?;
    }
    Ok(body)
}

/// Append a chunk within the limit.
pub(super) fn push_capped(body: &mut Vec<u8>, chunk: &[u8], limit: u64) -> Result<()> {
    if body.len() as u64 + chunk.len() as u64 > limit {
        bail!("cloud download is larger than the {limit}-byte limit");
    }
    body.extend_from_slice(chunk);
    Ok(())
}

/// A ZIP local-file-header magic — how a compressed Steam Cloud transfer starts.
pub(super) const ZIP_MAGIC: &[u8; 4] = b"PK\x03\x04";

/// Pull the single archived entry out of a ZIP container, or `None` if the bytes
/// aren't a readable one-entry archive.
pub(super) fn unzip_single_entry(body: &[u8], expected_len: Option<u32>) -> Option<Vec<u8>> {
    unzip_capped(body, expected_len, MAX_CLOUD_UNZIPPED)
}

/// [`unzip_single_entry`] with an explicit cap.
pub(super) fn unzip_capped(body: &[u8], expected_len: Option<u32>, max: u64) -> Option<Vec<u8>> {
    if !body.starts_with(ZIP_MAGIC) {
        return None;
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(body)).ok()?;
    if archive.is_empty() {
        return None;
    }
    let entry = archive.by_index(0).ok()?;
    let hint = expected_len.unwrap_or(0).min(MAX_CLOUD_PREALLOC) as usize;
    let mut out = Vec::with_capacity(hint);
    let limit = max.saturating_add(1);
    std::io::Read::read_to_end(&mut std::io::Read::take(entry, limit), &mut out).ok()?;
    if out.len() as u64 > max {
        tracing::warn!("Cloud file unzips past {max} bytes; keeping raw body");
        return None;
    }
    Some(out)
}

/// Unwrap a cloud download body that Steam stored compressed.
///
/// Steam ships such a file as a ZIP archive holding the single real save. The
/// decision is made against `expected_sha` — the hash Steam reports for the file's
/// *original* content — rather than the response's size fields, which Steam does not
/// reliably populate on this path: if the raw body already hashes to `expected_sha`
/// it is the file, and if the unzipped bytes do, the transfer was compressed. That
/// also means a save that genuinely *is* a zip can never be unwrapped by mistake.
///
/// With no hash to check against, fall back to the size fields (`raw_file_size`
/// differing from the stored `file_size` implies compression). If nothing matches,
/// the original bytes are returned and the mismatch logged — a suspect save beats no
/// save, and the sync stays alive for the remaining files.
pub(super) fn maybe_decompress(
    body: Vec<u8>,
    stored_size: u32,
    raw_file_size: u32,
    expected_sha: Option<&str>,
    filename: &str,
) -> Vec<u8> {
    let raw_len = (raw_file_size > 0).then_some(raw_file_size);

    if let Some(sha) = expected_sha.map(str::trim).filter(|s| !s.is_empty()) {
        if hash_eq(&hex_lower(&Sha1::digest(&body)), sha) {
            return body; // stored uncompressed — already the real file
        }
        match unzip_single_entry(&body, raw_len) {
            Some(inner) if hash_eq(&hex_lower(&Sha1::digest(&inner)), sha) => return inner,
            _ => {
                tracing::warn!(
                    "cloud file '{filename}' did not match its expected hash {sha} either as \
                     received or decompressed; writing it as received"
                );
                return body;
            }
        }
    }

    // No hash available: compression is implied by the two size fields disagreeing.
    if raw_file_size > 0 && raw_file_size != stored_size {
        if let Some(inner) = unzip_single_entry(&body, raw_len) {
            if inner.len() as u32 != raw_file_size {
                tracing::warn!(
                    "cloud file '{filename}' decompressed to {} bytes, expected {raw_file_size}",
                    inner.len()
                );
            }
            return inner;
        }
        tracing::warn!("cloud file '{filename}' looked compressed but is not a readable zip");
    }
    body
}

/// Build the request URL for a Steam Cloud HTTP transfer from the scheme flag and
/// host/path the service returned, rejecting the empty host/path Steam sends when a
/// transfer slot is unavailable. `what` names the originating RPC for the error.
pub(super) fn cloud_transfer_url(
    use_https: bool,
    host: &str,
    path: &str,
    what: &str,
) -> Result<String> {
    if host.is_empty() || path.is_empty() {
        return Err(anyhow!("{what} returned empty URL host/path"));
    }
    let scheme = if use_https { "https" } else { "http" };
    Ok(format!("{scheme}://{host}{path}"))
}

pub(super) fn build_header_map<'a>(
    headers: impl Iterator<Item = (&'a str, &'a str)>,
) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        if name.is_empty() {
            continue;
        }

        let header_name = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid header name '{name}'"))?;
        let header_value = HeaderValue::from_str(value)
            .with_context(|| format!("invalid header value for '{name}'"))?;
        map.insert(header_name, header_value);
    }
    Ok(map)
}
