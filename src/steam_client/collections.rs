//! SteamClient RPC methods for Steam library collections, backed by the
//! CloudConfigStore service (namespace `1`). See [`super::cloudconfig`] for the
//! generated protobuf message types and their RPC wiring.

use anyhow::{Context, Result};

use super::cloudconfig::{
    CCloudConfigStore_Download_Request, CCloudConfigStore_Entry, CCloudConfigStore_NamespaceData,
    CCloudConfigStore_NamespaceVersion, CCloudConfigStore_Upload_Request,
};
use super::SteamClient;
use steam_vent::ConnectionTrait;

/// The user-collections namespace in CloudConfigStore.
const COLLECTIONS_NAMESPACE: u32 = 1;

/// A decoded snapshot of the `user-collections` namespace as stored in Steam's
/// cloud: its monotonically-increasing `version` plus the raw key/value entries.
/// `value` is `None` for a tombstoned (deleted) entry.
#[derive(Debug, Clone, Default)]
pub struct RemoteNamespace {
    pub version: u64,
    /// `(key, Some(value_json))`, or `(key, None)` when the entry is a deletion.
    pub entries: Vec<(String, Option<String>)>,
}

impl SteamClient {
    /// Download the current `user-collections` namespace from Steam's cloud.
    ///
    /// Requires a logged-in (non-anonymous) session.
    pub async fn download_collections(&self) -> Result<RemoteNamespace> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut req = CCloudConfigStore_Download_Request::new();
        let mut nv = CCloudConfigStore_NamespaceVersion::new();
        nv.set_enamespace(COLLECTIONS_NAMESPACE);
        nv.set_version(0);
        req.versions.push(nv);

        let resp = connection
            .service_method(req)
            .await
            .context("CloudConfigStore.Download failed")?;

        let mut out = RemoteNamespace::default();
        if let Some(data) = resp
            .data
            .iter()
            .find(|d| d.enamespace() == COLLECTIONS_NAMESPACE)
        {
            out.version = data.version();
            for entry in &data.entries {
                let key = entry.key().to_string();
                if key.is_empty() {
                    continue;
                }
                let value = if entry.is_deleted() {
                    None
                } else {
                    Some(entry.value().to_string())
                };
                out.entries.push((key, value));
            }
        }
        Ok(out)
    }

    /// Upload the given `user-collections` entries to Steam's cloud at the
    /// provided base `version`, returning the new namespace version. An entry
    /// with `None` value is uploaded as a deletion (tombstone).
    ///
    /// Requires a logged-in (non-anonymous) session.
    pub async fn upload_collections(
        &self,
        version: u64,
        entries: Vec<(String, Option<String>)>,
    ) -> Result<u64> {
        let connection = self
            .connection
            .as_ref()
            .context("steam connection not initialized")?;

        let mut nd = CCloudConfigStore_NamespaceData::new();
        nd.set_enamespace(COLLECTIONS_NAMESPACE);
        nd.set_version(version);
        for (key, value) in entries {
            let mut entry = CCloudConfigStore_Entry::new();
            entry.set_key(key);
            match value {
                Some(v) => entry.set_value(v),
                None => entry.set_is_deleted(true),
            }
            nd.entries.push(entry);
        }

        let mut req = CCloudConfigStore_Upload_Request::new();
        req.data.push(nd);

        let resp = connection
            .service_method(req)
            .await
            .context("CloudConfigStore.Upload failed")?;

        let new_version = resp
            .versions
            .iter()
            .find(|v| v.enamespace() == COLLECTIONS_NAMESPACE)
            .map(|v| v.version())
            .unwrap_or(version);
        Ok(new_version)
    }
}
