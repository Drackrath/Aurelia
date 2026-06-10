use std::io::{Cursor, Read};
use zip::ZipArchive;

use crate::{
    crypto::aes256::{self, IV_LENGTH},
    utils::lzma,
    Error,
};

pub async fn decrypt_and_decompress(data: &mut [u8], key: [u8; 32]) -> Result<Vec<u8>, Error> {
    if data.len() <= IV_LENGTH {
        return Err(Error::Eof("data is too small".to_string()));
    }

    let decrypted = aes256::decrypt_cbc_with_iv_extraction(data, key)?;
    if lzma::is_vz(&decrypted) {
        // Legacy LZMA container.
        Ok(lzma::decompress(&decrypted).await?)
    } else if lzma::is_vsz(&decrypted) {
        // Newer Zstandard container.
        Ok(lzma::decompress_vsz(&decrypted)?)
    } else {
        // Otherwise the payload is a single-entry zip.
        let cursor = Cursor::new(decrypted);
        let mut buffer = Vec::new();
        ZipArchive::new(cursor)?
            .by_index(0)?
            .read_to_end(&mut buffer)?;
        Ok(buffer)
    }
}
