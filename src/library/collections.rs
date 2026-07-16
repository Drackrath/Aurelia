//! Local store, wire format, and cloud sync for Steam library collections
//! (the "categories" you group games into in the Steam client).
//!
//! Collections live in Steam's cloud under the CloudConfigStore `user-collections`
//! namespace: each collection is one entry keyed `user-collections.<id>` whose
//! `value` is a JSON blob `{ id, name, added:[appid], removed:[appid],
//! filterSpec? }`. Aurelia keeps a **local working copy** in
//! `config_dir()/collections.json` that is edited offline; changes reach Steam
//! only via [`pull`], [`push`], and [`sync`].
//!
//! Collections come in two flavours:
//! - **Static**: an explicit membership list (`added` minus `removed`). These are
//!   the ones Aurelia can create and edit.
//! - **Dynamic**: membership is computed by Steam from a `filterSpec` (tags,
//!   platforms, …). Aurelia round-trips these opaquely — it never edits or
//!   fabricates a `filterSpec` — so a `sync` won't clobber them.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::steam_client::{RemoteNamespace, SteamClient};

/// Key prefix for every collection entry in the CloudConfigStore namespace.
const KEY_PREFIX: &str = "user-collections.";

/// Built-in collection ids that Steam manages specially. They can be cleared
/// (add/remove members) but never deleted.
const BUILTIN_IDS: &[&str] = &["favorite", "hidden"];

/// A single Steam library collection (a named group of games).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Collection {
    /// Stable id, e.g. `uc-1a2b3c4d`, or a built-in id (`favorite`/`hidden`).
    pub id: String,
    /// User-visible name.
    pub name: String,
    /// App ids explicitly added to the collection.
    #[serde(default)]
    pub added: Vec<u32>,
    /// App ids explicitly removed (tombstoned) from the collection.
    #[serde(default)]
    pub removed: Vec<u32>,
    /// Opaque dynamic-collection filter. When present the collection is
    /// *dynamic* and is round-tripped verbatim; Aurelia never edits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_spec: Option<Value>,
    /// Marked for deletion; pushed to Steam as a tombstone, then dropped locally.
    #[serde(default)]
    pub deleted: bool,
}

impl Collection {
    /// Whether this is a dynamic (filter-driven) collection.
    pub fn is_dynamic(&self) -> bool {
        self.filter_spec.is_some()
    }

    /// Whether this is a built-in (`favorite`/`hidden`) collection.
    pub fn is_builtin(&self) -> bool {
        BUILTIN_IDS.contains(&self.id.as_str())
    }

    /// Static membership test: a member iff explicitly added and not removed.
    pub fn contains(&self, app_id: u32) -> bool {
        self.added.contains(&app_id) && !self.removed.contains(&app_id)
    }

    /// Convert to a Steam CloudConfigStore entry: `(key, Option<value_json>)`.
    /// A `deleted` collection yields `None` (a deletion tombstone).
    pub fn to_entry(&self) -> (String, Option<String>) {
        let key = format!("{KEY_PREFIX}{}", self.id);
        if self.deleted {
            return (key, None);
        }
        let mut obj = serde_json::Map::new();
        obj.insert("id".into(), Value::from(self.id.clone()));
        obj.insert("name".into(), Value::from(self.name.clone()));
        obj.insert("added".into(), Value::from(self.added.clone()));
        obj.insert("removed".into(), Value::from(self.removed.clone()));
        // Round-trip a dynamic collection's filter opaquely; never fabricate one.
        if let Some(filter) = &self.filter_spec {
            obj.insert("filterSpec".into(), filter.clone());
        }
        let value = Value::Object(obj).to_string();
        (key, Some(value))
    }

    /// Parse a Steam CloudConfigStore entry (`key` + its JSON `value`) into a
    /// [`Collection`]. Unknown/dynamic filters are preserved in `filter_spec`.
    pub fn from_entry(key: &str, value_json: &str) -> Result<Collection> {
        let v: Value = serde_json::from_str(value_json)
            .with_context(|| format!("collection entry {key} has invalid JSON value"))?;
        let key_id = key.strip_prefix(KEY_PREFIX).unwrap_or(key).to_string();
        let id = v
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.is_empty())
            .unwrap_or(key_id);
        let name = v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Ok(Collection {
            id,
            name,
            added: parse_app_ids(v.get("added")),
            removed: parse_app_ids(v.get("removed")),
            filter_spec: v.get("filterSpec").cloned(),
            deleted: false,
        })
    }
}

/// Parse a JSON array of numeric app ids into a `Vec<u32>`, ignoring non-numbers.
fn parse_app_ids(value: Option<&Value>) -> Vec<u32> {
    value
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|x| x.as_u64().map(|n| n as u32)).collect())
        .unwrap_or_default()
}

/// The on-disk local working copy of all collections plus the last-known Steam
/// namespace version (used for conflict detection on push).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollectionsStore {
    /// Last CloudConfigStore version we synced against. `0` means never synced.
    #[serde(default)]
    pub namespace_version: u64,
    #[serde(default)]
    pub collections: Vec<Collection>,
}

/// Path to the local collections store.
fn store_path() -> Result<std::path::PathBuf> {
    Ok(crate::core::config::config_dir()?.join("collections.json"))
}

impl CollectionsStore {
    /// Load the local store. A missing file yields an empty store.
    pub fn load() -> Result<CollectionsStore> {
        let path = store_path()?;
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("failed parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(CollectionsStore::default()),
            Err(e) => Err(e).with_context(|| format!("failed reading {}", path.display())),
        }
    }

    /// Persist the local store (pretty JSON).
    pub fn save(&self) -> Result<()> {
        let path = store_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let text = serde_json::to_string_pretty(self).context("failed serializing collections")?;
        std::fs::write(&path, text).with_context(|| format!("failed writing {}", path.display()))
    }

    /// Resolve a collection by exact id or case-insensitive name. Errors on
    /// no-match or an ambiguous (duplicate) name.
    pub fn resolve(&self, name_or_id: &str) -> Result<&Collection> {
        let idx = self.resolve_index(name_or_id)?;
        Ok(&self.collections[idx])
    }

    /// Index of the collection matching `name_or_id` (exact id, else
    /// case-insensitive unique name). Skips `deleted` collections.
    fn resolve_index(&self, name_or_id: &str) -> Result<usize> {
        // Exact id match wins outright.
        if let Some(i) = self
            .collections
            .iter()
            .position(|c| !c.deleted && c.id == name_or_id)
        {
            return Ok(i);
        }
        let wanted = name_or_id.to_ascii_lowercase();
        let matches: Vec<usize> = self
            .collections
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.deleted && c.name.to_ascii_lowercase() == wanted)
            .map(|(i, _)| i)
            .collect();
        match matches.as_slice() {
            [] => bail!("no collection named or with id '{name_or_id}'"),
            [i] => Ok(*i),
            _ => bail!(
                "'{name_or_id}' is ambiguous: {} collections share that name — use the id instead",
                matches.len()
            ),
        }
    }

    // ---- Local (offline) operations -------------------------------------

    /// Create a new **static** collection and save. Returns its generated id.
    pub fn create(&mut self, name: &str) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            bail!("collection name must not be empty");
        }
        let id = format!("uc-{:08x}", rand::random::<u32>());
        self.collections.push(Collection {
            id: id.clone(),
            name: name.to_string(),
            added: Vec::new(),
            removed: Vec::new(),
            filter_spec: None,
            deleted: false,
        });
        self.save()?;
        Ok(id)
    }

    /// Mark a collection deleted and save. Built-ins can't be deleted (only
    /// cleared); dynamic collections aren't editable via the CLI.
    pub fn delete(&mut self, name_or_id: &str) -> Result<()> {
        let idx = self.resolve_index(name_or_id)?;
        let c = &self.collections[idx];
        if c.is_builtin() {
            bail!(
                "'{}' is a built-in collection and can't be deleted — remove its games instead",
                c.name
            );
        }
        if c.is_dynamic() {
            bail!(
                "'{}' is a dynamic (filter-based) collection; edit it in the Steam client",
                c.name
            );
        }
        self.collections[idx].deleted = true;
        self.save()
    }

    /// Rename a collection and save.
    pub fn rename(&mut self, name_or_id: &str, new_name: &str) -> Result<()> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            bail!("new collection name must not be empty");
        }
        let idx = self.resolve_index(name_or_id)?;
        self.collections[idx].name = new_name.to_string();
        self.save()
    }

    /// Add app ids to a static collection and save.
    pub fn add(&mut self, name_or_id: &str, app_ids: &[u32]) -> Result<()> {
        let idx = self.resolve_index(name_or_id)?;
        self.ensure_static(idx)?;
        let c = &mut self.collections[idx];
        for &app in app_ids {
            c.removed.retain(|&x| x != app);
            if !c.added.contains(&app) {
                c.added.push(app);
            }
        }
        self.save()
    }

    /// Remove app ids from a static collection (tombstone them) and save.
    pub fn remove(&mut self, name_or_id: &str, app_ids: &[u32]) -> Result<()> {
        let idx = self.resolve_index(name_or_id)?;
        self.ensure_static(idx)?;
        let c = &mut self.collections[idx];
        for &app in app_ids {
            c.added.retain(|&x| x != app);
            if !c.removed.contains(&app) {
                c.removed.push(app);
            }
        }
        self.save()
    }

    /// Error if the collection at `idx` is dynamic (not editable via the CLI).
    fn ensure_static(&self, idx: usize) -> Result<()> {
        let c = &self.collections[idx];
        if c.is_dynamic() {
            bail!(
                "'{}' is a dynamic (filter-based) collection; its membership is computed by \
                 Steam and can't be edited here — manage it in the Steam client",
                c.name
            );
        }
        Ok(())
    }

    // ---- Sync -----------------------------------------------------------

    /// Merge a downloaded remote snapshot into the local store (pure; no I/O).
    ///
    /// Semantics: union `added`/`removed` per id; remote wins for `name` and
    /// `filter_spec`; brand-new remote collections are added; a remote deletion
    /// (tombstone) drops the local collection. Sets `namespace_version` from the
    /// remote.
    fn apply_remote(&mut self, remote: RemoteNamespace) {
        for (key, value) in remote.entries {
            let id = key.strip_prefix(KEY_PREFIX).unwrap_or(&key).to_string();
            match value {
                // Deletion tombstone: drop any local copy.
                None => self.collections.retain(|c| c.id != id),
                Some(value_json) => {
                    let incoming = match Collection::from_entry(&key, &value_json) {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::warn!("skipping unparseable collection {key}: {e:#}");
                            continue;
                        }
                    };
                    match self.collections.iter_mut().find(|c| c.id == incoming.id) {
                        Some(local) => {
                            // Union memberships; remote wins for name/filter.
                            for a in &incoming.added {
                                if !local.added.contains(a) {
                                    local.added.push(*a);
                                }
                            }
                            for r in &incoming.removed {
                                if !local.removed.contains(r) {
                                    local.removed.push(*r);
                                }
                            }
                            local.name = incoming.name;
                            local.filter_spec = incoming.filter_spec;
                        }
                        None => self.collections.push(incoming),
                    }
                }
            }
        }
        self.namespace_version = remote.version;
    }
}

/// Pull remote collections from Steam, merge into `store`, and save.
pub async fn pull(store: &mut CollectionsStore, client: &SteamClient) -> Result<()> {
    let remote = client
        .download_collections()
        .await
        .context("failed downloading collections from Steam")?;
    store.apply_remote(remote);
    store.save()?;
    Ok(())
}

/// Push every local collection to Steam, then save. On a version conflict the
/// error tells the user to `pull` first.
pub async fn push(store: &mut CollectionsStore, client: &SteamClient) -> Result<()> {
    let entries: Vec<(String, Option<String>)> =
        store.collections.iter().map(Collection::to_entry).collect();

    let new_version = match client
        .upload_collections(store.namespace_version, entries)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            let text = e.to_string().to_ascii_lowercase();
            if text.contains("version") || text.contains("conflict") || text.contains("out of date")
            {
                bail!(
                    "Steam rejected the upload — your local collections are out of date. \
                     Run `aurelia collections pull` first, then push again. ({e})"
                );
            }
            return Err(e).context("failed uploading collections to Steam");
        }
    };

    store.namespace_version = new_version;
    // Deleted collections are now tombstoned server-side; drop them locally.
    store.collections.retain(|c| !c.deleted);
    store.save()?;
    Ok(())
}

/// Full sync: `pull` (merge remote in) then `push` (send the merged result).
pub async fn sync(store: &mut CollectionsStore, client: &SteamClient) -> Result<()> {
    pull(store, client).await?;
    push(store, client).await?;
    Ok(())
}

#[cfg(test)]
#[path = "collections_tests.rs"]
mod tests;
