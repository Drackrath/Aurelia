use lzma_rs::decompress::raw::{LzmaDecoder, LzmaParams, LzmaProperties};
use std::io::{Cursor, SeekFrom};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    task,
};

use crate::Error;

const VZ_HEADER: u16 = 0x5A56;
const VZ_FOOTER: u16 = 0x767A;
const VZ_VERSION: char = 'a';
const VZ_HEADER_LENGTH: usize = 7;
const VZ_FOOTER_LENGTH: usize = 10;

// Steam's newer Zstandard chunk container. Layout mirrors VZ:
//   header: "VSZ" + version 'a' + crc32(4) = 8 bytes
//   body:   a single raw Zstandard frame (self-delimiting; carries its own size)
//   footer: trailing fields ending in the "zsv" magic
const VSZ_HEADER: [u8; 3] = [b'V', b'S', b'Z'];
const VSZ_VERSION: u8 = b'a';
const VSZ_HEADER_LENGTH: usize = 8;
const VSZ_FOOTER_MAGIC: [u8; 3] = [b'z', b's', b'v'];
const VSZ_FOOTER_LENGTH: usize = 11;

pub fn is_vz(data: &[u8]) -> bool {
    data.len() >= 2 && u16::from_le_bytes([data[0], data[1]]) == VZ_HEADER
}

pub fn is_vsz(data: &[u8]) -> bool {
    data.len() >= 4 && data[0..3] == VSZ_HEADER && data[3] == VSZ_VERSION
}

/// Decompress a Steam "VSZ" (Zstandard) chunk container.
pub fn decompress_vsz(data: &[u8]) -> Result<Vec<u8>, Error> {
    if !is_vsz(data) {
        return Err(Error::Decompress("expecting VSZ header".to_string()));
    }
    if data.len() < VSZ_HEADER_LENGTH + VSZ_FOOTER_LENGTH {
        return Err(Error::Eof("VSZ chunk too small".to_string()));
    }
    if data[data.len() - 3..] != VSZ_FOOTER_MAGIC {
        return Err(Error::Decompress("expecting zsv at end of stream".to_string()));
    }

    // The Zstandard frame sits between the fixed header and footer. It is
    // self-delimiting and carries its own content size, so ruzstd stops at the end of
    // the frame; the trailing footer bytes are simply not consumed.
    let frame = &data[VSZ_HEADER_LENGTH..data.len() - VSZ_FOOTER_LENGTH];
    let mut decoder = ruzstd::StreamingDecoder::new(std::io::Cursor::new(frame))
        .map_err(|e| Error::Decompress(format!("zstd frame: {e}")))?;
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut out)?;
    Ok(out)
}

pub async fn decompress(data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut cursor = Cursor::new(data);
    if cursor.read_u16_le().await? != VZ_HEADER {
        return Err(Error::Eof("expecting VZ header".to_string()));
    }

    if cursor.read_u8().await? != VZ_VERSION as u8 {
        return Err(Error::Eof("expecting VZ header".to_string()));
    }

    let mut properties = [0u8; 5];
    cursor.seek(SeekFrom::Current(4)).await?; // skip crc32
    cursor.read_exact(&mut properties).await?;

    let buffer_size =
        cursor.get_ref().len() - properties.len() - VZ_HEADER_LENGTH - VZ_FOOTER_LENGTH;
    let mut buffer = vec![0u8; buffer_size];
    cursor.read_exact(&mut buffer).await?;

    let decompressed_crc32 = cursor.read_u32_le().await?;
    let decompressed_size = cursor.read_u32_le().await?;

    if cursor.read_u16_le().await? != VZ_FOOTER {
        return Err(Error::Eof("expecting VZ at end of stream".to_string()));
    }

    let decompressed_data = task::spawn_blocking(move || -> Result<Vec<u8>, Error> {
        let mut decompressed = Vec::with_capacity(decompressed_size as usize);

        let lc = (properties[0] % 9) as u32;
        let remainder = (properties[0] / 9) as u32;
        let lp = remainder % 5;
        let pb = remainder / 5;

        let mut dict_size = 0u32;

        for i in 0..4 {
            dict_size += (properties[1 + i] as u32) << (i * 8);
        }

        LzmaDecoder::new(
            LzmaParams::new(
                LzmaProperties { lc, lp, pb },
                dict_size,
                Some(decompressed_size as u64),
            ),
            None,
        )?
        .decompress(&mut Cursor::new(buffer), &mut decompressed)?;

        Ok(decompressed)
    })
    .await??;

    if decompressed_crc32 != crc32fast::hash(&decompressed_data) {
        return Err(Error::Decompress("crc32 mismatch".to_string()));
    }

    Ok(decompressed_data)
}
