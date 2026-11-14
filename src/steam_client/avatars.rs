//! Friend avatars for stand-in games.
//!
//! Steam CDN JPEGs, decoded to RGBA.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// Avatar sizes games ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AvatarSize {
    /// 32 by 32.
    Small,
    /// 64 by 64.
    Medium,
    /// 184 by 184.
    Large,
}

impl AvatarSize {
    pub fn from_wire(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Small),
            1 => Some(Self::Medium),
            2 => Some(Self::Large),
            _ => None,
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Self::Small => "",
            Self::Medium => "_medium",
            Self::Large => "_full",
        }
    }
}

/// Decoded RGBA image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Avatar {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Hex hash, None when unset.
pub fn avatar_hex(hash: &[u8]) -> Option<String> {
    if hash.is_empty() || hash.iter().all(|&b| b == 0) {
        return None;
    }
    Some(hash.iter().map(|b| format!("{b:02x}")).collect())
}

/// CDN URL for one size.
pub fn avatar_url(hex: &str, size: AvatarSize) -> String {
    format!("https://avatars.steamstatic.com/{hex}{}.jpg", size.suffix())
}

/// Largest avatar we accept.
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_SIDE: usize = 1024;

/// JPEG bytes to RGBA.
pub fn decode_jpeg(bytes: &[u8]) -> Result<Avatar> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(std::io::Cursor::new(bytes), options);
    decoder
        .decode_headers()
        .map_err(|e| anyhow!("bad avatar JPEG: {e:?}"))?;
    let info = decoder.info().context("avatar JPEG has no size")?;
    let (w, h) = (usize::from(info.width), usize::from(info.height));
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        bail!("avatar JPEG is {w}x{h}");
    }
    let rgba = decoder
        .decode()
        .map_err(|e| anyhow!("bad avatar JPEG: {e:?}"))?;
    if rgba.len() != w * h * 4 {
        bail!("avatar JPEG decoded to {} bytes", rgba.len());
    }
    Ok(Avatar {
        width: u32::try_from(w)?,
        height: u32::try_from(h)?,
        rgba,
    })
}

type Cache = HashMap<(String, AvatarSize), Arc<Avatar>>;

/// Decoded avatars by hash and size.
static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn cached(hex: &str, size: AvatarSize) -> Option<Arc<Avatar>> {
    CACHE
        .lock()
        .unwrap()
        .as_ref()?
        .get(&(hex.to_owned(), size))
        .cloned()
}

fn remember(hex: &str, size: AvatarSize, avatar: Arc<Avatar>) {
    let mut cache = CACHE.lock().unwrap();
    let cache = cache.get_or_insert_with(HashMap::new);
    // Bound memory; avatars refetch cheaply.
    if cache.len() >= 512 {
        cache.clear();
    }
    cache.insert((hex.to_owned(), size), avatar);
}

/// Cached, else downloaded and decoded.
pub async fn fetch_avatar(hex: &str, size: AvatarSize) -> Result<Arc<Avatar>> {
    if let Some(hit) = cached(hex, size) {
        return Ok(hit);
    }
    let http = crate::core::net::http_client(Duration::from_secs(15))?;
    let response = http
        .get(avatar_url(hex, size))
        .send()
        .await
        .context("avatar download failed")?
        .error_for_status()
        .context("avatar download failed")?;
    let bytes = response.bytes().await.context("avatar download failed")?;
    if bytes.len() > MAX_BYTES {
        bail!("avatar is {} bytes", bytes.len());
    }
    let avatar = Arc::new(tokio::task::spawn_blocking(move || decode_jpeg(&bytes)).await??);
    remember(hex, size, Arc::clone(&avatar));
    Ok(avatar)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_urls() {
        assert_eq!(avatar_hex(&[0; 20]), None);
        assert_eq!(avatar_hex(&[]), None);
        let hex = avatar_hex(&[0xAB, 0x01]).unwrap();
        assert_eq!(hex, "ab01");
        assert_eq!(
            avatar_url(&hex, AvatarSize::Small),
            "https://avatars.steamstatic.com/ab01.jpg"
        );
        assert!(avatar_url(&hex, AvatarSize::Medium).ends_with("ab01_medium.jpg"));
        assert!(avatar_url(&hex, AvatarSize::Large).ends_with("ab01_full.jpg"));
        assert_eq!(AvatarSize::from_wire(2), Some(AvatarSize::Large));
        assert_eq!(AvatarSize::from_wire(3), None);
    }

    #[test]
    fn decodes_jpeg_to_rgba() {
        let jpeg = include_bytes!("avatar_test_4x2.jpg");
        let avatar = decode_jpeg(jpeg).unwrap();
        assert_eq!((avatar.width, avatar.height), (4, 2));
        assert_eq!(avatar.rgba.len(), 4 * 2 * 4);
        let px = &avatar.rgba[..4];
        assert!(
            px[0] > 240 && px[1] < 16 && px[2] < 16 && px[3] == 255,
            "{px:?}"
        );
        assert!(decode_jpeg(b"not a jpeg").is_err());
    }

    #[test]
    fn cache_hits_skip_download() {
        let avatar = Arc::new(Avatar {
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 4],
        });
        remember("cafe", AvatarSize::Small, Arc::clone(&avatar));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let hit = rt
            .block_on(fetch_avatar("cafe", AvatarSize::Small))
            .unwrap();
        assert_eq!(hit, avatar);
        assert!(cached("cafe", AvatarSize::Large).is_none());
    }
}
