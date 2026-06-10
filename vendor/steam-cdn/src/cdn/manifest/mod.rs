use bytes::{Buf, Bytes};
use error::ManifestError;
use file::{ChunkData, ManifestFile};
use std::sync::Arc;
use std::{
    io::{Cursor, Read},
    str,
};
use steam_vent::proto::{
    content_manifest::{ContentManifestMetadata, ContentManifestPayload},
    protobuf::Message,
};
use zip::ZipArchive;

use super::inner::InnerClient;
use crate::{crypto::aes256, utils::base64::base64_decode};

mod buf;
pub mod error;
pub mod file;

const PROTOBUF_PAYLOAD_MAGIC: u32 = 0x71F617D0;
const PROTOBUF_METADATA_MAGIC: u32 = 0x1F4812BE;
const PROTOBUF_SIGNATURE_MAGIC: u32 = 0x1B81B817;
const PROTOBUF_ENDOFMANIFEST_MAGIC: u32 = 0x32C415AB;

#[derive(Debug)]
pub struct DepotManifest {
    app_id: u32,
    depot_id: u32,
    manifest_gid: u64,
    creatime_time: u32,
    filenames_encrypted: bool,
    original_size: u64,
    compressed_size: u64,
    files: Vec<ManifestFile>,
}

impl DepotManifest {
    pub fn depot_id(&self) -> u32 {
        self.depot_id
    }

    pub fn manifest_gid(&self) -> u64 {
        self.manifest_gid
    }

    pub fn creatime_time(&self) -> u32 {
        self.creatime_time
    }

    pub fn filenames_encrypted(&self) -> bool {
        self.filenames_encrypted
    }

    pub fn original_size(&self) -> u64 {
        self.original_size
    }

    pub fn compressed_size(&self) -> u64 {
        self.compressed_size
    }

    pub fn files(&self) -> &Vec<ManifestFile> {
        &self.files
    }

    pub fn decrypt_filenames(&mut self, key: [u8; 32]) -> Result<(), ManifestError> {
        if self.filenames_encrypted {
            for file in &mut self.files {
                let mut encrypted = base64_decode(file.filename.as_bytes())?;
                file.filename = str::from_utf8(&aes256::decrypt_cbc_with_iv_extraction(
                    &mut encrypted[..],
                    key,
                )?)?
                .to_string();
            }
            self.filenames_encrypted = false;
        }
        Ok(())
    }

    pub(crate) fn deserialize(
        client: Arc<InnerClient>,
        app_id: u32,
        depot_id: u32,
        manifest_gid: u64,
        data: &[u8],
    ) -> Result<Self, ManifestError> {
        let mut buffer = Vec::new();
        let is_zip = data.len() > 2 && data[0] == 0x50 && data[1] == 0x4B;

        let raw_data = if is_zip {
            let cursor = Cursor::new(data);
            match ZipArchive::new(cursor) {
                Ok(mut archive) if archive.len() > 0 => {
                    if let Ok(mut file) = archive.by_index(0) {
                        if file.read_to_end(&mut buffer).is_ok() {
                            &buffer[..]
                        } else {
                            data
                        }
                    } else {
                        data
                    }
                }
                _ => data,
            }
        } else {
            data
        };

        let mut bytes = Bytes::from(raw_data.to_vec());
        let mut payload = None;
        let mut metadata = None;

        // Try parsing using the magic-wrapped sequence first
        if raw_data.len() > 8
            && u32::from_le_bytes(raw_data[0..4].try_into().unwrap()) == PROTOBUF_PAYLOAD_MAGIC
        {
            while bytes.remaining() >= 8 {
                let magic = bytes.get_u32_le();
                let len = bytes.get_u32_le() as usize;

                if bytes.remaining() < len {
                    break;
                }

                let body = bytes.copy_to_bytes(len);
                if magic == PROTOBUF_PAYLOAD_MAGIC {
                    payload = ContentManifestPayload::parse_from_bytes(&body).ok();
                } else if magic == PROTOBUF_METADATA_MAGIC {
                    metadata = ContentManifestMetadata::parse_from_bytes(&body).ok();
                }
            }
        }

        if payload.is_none() {
            // Fallback: Check for offset 8 or 4 as suggested by user
            let offset = if raw_data.len() > 8 && raw_data[8] == 0x0A {
                8
            } else if raw_data.len() > 4 && raw_data[4] == 0x0A {
                4
            } else {
                0
            };

            payload = Some(ContentManifestPayload::parse_from_bytes(&raw_data[offset..])?);
        }

        let payload = payload.expect("Payload should be present at this point");

        let final_depot_id = metadata.as_ref().map(|m| m.depot_id()).unwrap_or(depot_id);
        let final_manifest_gid = metadata.as_ref().map(|m| m.gid_manifest()).unwrap_or(manifest_gid);
        let final_creation_time = metadata.as_ref().map(|m| m.creation_time()).unwrap_or(0);
        let final_filenames_encrypted = metadata.as_ref().map(|m| m.filenames_encrypted()).unwrap_or(false);
        let final_original_size = metadata.as_ref().map(|m| m.cb_disk_original()).unwrap_or(0);
        let final_compressed_size = metadata.as_ref().map(|m| m.cb_disk_compressed()).unwrap_or(0);

        Ok(Self {
            app_id,
            depot_id: final_depot_id,
            manifest_gid: final_manifest_gid,
            creatime_time: final_creation_time,
            filenames_encrypted: final_filenames_encrypted,
            original_size: final_original_size,
            compressed_size: final_compressed_size,
            files: payload
                .mappings
                .into_iter()
                .map(|map| ManifestFile {
                    inner: client.clone(),
                    app_id,
                    depot_id: final_depot_id,
                    filename: map.filename().to_string(),
                    size: map.size(),
                    flags: map.flags(),
                    sha_filename: map.sha_filename().to_vec(),
                    sha_content: map.sha_content().to_vec(),
                    chunks: map
                        .chunks
                        .iter()
                        .map(|chunk| ChunkData {
                            sha: chunk.sha().to_vec(),
                            crc: chunk.crc(),
                            offset: chunk.offset(),
                            original_size: chunk.cb_original(),
                            compressed_size: chunk.cb_compressed(),
                        })
                        .collect::<Vec<ChunkData>>(),
                    linktarget: map.linktarget().to_string(),
                })
                .collect::<Vec<ManifestFile>>(),
        })
    }
}
