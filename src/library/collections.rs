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

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::steam_client::{RemoteNamespace, SteamClient};

/// Key prefix for every collection entry in the CloudConfigStore namespace.
const KEY_PREFIX: &str = "user-collections.";

/// Built-in collection ids that Steam manages specially. They can be cleared
/// (add/remove members) but never deleted.
const BUILTIN_IDS: &[&str] = &["favorite", "hidden"];

/// Steam's default built-in names.
fn builtin_default_name(id: &str) -> Option<&'static str> {
    match id {
        "favorite" => Some("Favorites"),
        "hidden" => Some("Hidden"),
        _ => None,
    }
}

/// Serializes load-edit-save within this process.
static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Pending ids with state, per collection.
type PushSnapshot = Vec<(String, Vec<(u32, bool)>)>;

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
    /// App ids edited locally since push.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<u32>,
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
            pending: Vec::new(),
        })
    }
}

/// Parse a JSON array of numeric app ids into a `Vec<u32>`, ignoring non-numbers.
fn parse_app_ids(value: Option<&Value>) -> Vec<u32> {
    value
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_u64().map(|n| n as u32))
                .collect()
        })
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
    /// Store format; 0 predates `pending`.
    #[serde(default)]
    pub format: u32,
}

/// Format with `pending` tracking.
pub const STORE_FORMAT: u32 = 1;

/// Explicit membership: added, removed, neither.
fn membership(added: &[u32], removed: &[u32], app: u32) -> Option<bool> {
    if removed.contains(&app) {
        Some(false)
    } else if added.contains(&app) {
        Some(true)
    } else {
        None
    }
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
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(CollectionsStore {
                format: STORE_FORMAT,
                ..CollectionsStore::default()
            }),
            Err(e) => Err(e).with_context(|| format!("failed reading {}", path.display())),
        }
    }

    /// Persist the local store (pretty JSON).
    pub fn save(&self) -> Result<()> {
        let path = store_path()?;
        let text = serde_json::to_string_pretty(self).context("failed serializing collections")?;
        crate::core::atomic::write_atomic(&path, text.as_bytes())
    }

    /// Locked load-edit-save; never across network.
    pub fn edit<T>(f: impl FnOnce(&mut CollectionsStore) -> Result<T>) -> Result<(T, Self)> {
        let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = Self::load()?;
        let out = f(&mut store)?;
        store.save()?;
        Ok((out, store))
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

    /// Index, creating a missing built-in.
    fn resolve_or_create_builtin(&mut self, name_or_id: &str) -> Result<usize> {
        match self.resolve_index(name_or_id) {
            Ok(i) => Ok(i),
            Err(e) => {
                let Some(name) = builtin_default_name(name_or_id) else {
                    return Err(e);
                };
                // Remote name wins on pull.
                self.collections.push(Collection {
                    id: name_or_id.to_string(),
                    name: name.to_string(),
                    added: Vec::new(),
                    removed: Vec::new(),
                    filter_spec: None,
                    deleted: false,
                    pending: Vec::new(),
                });
                Ok(self.collections.len() - 1)
            }
        }
    }

    /// Create a static collection; returns id.
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
            pending: Vec::new(),
        });
        Ok(id)
    }

    /// Mark deleted; refuses built-ins, dynamic.
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
        Ok(())
    }

    /// Rename a collection.
    pub fn rename(&mut self, name_or_id: &str, new_name: &str) -> Result<()> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            bail!("new collection name must not be empty");
        }
        let idx = self.resolve_index(name_or_id)?;
        self.collections[idx].name = new_name.to_string();
        Ok(())
    }

    /// Add app ids to a collection.
    pub fn add(&mut self, name_or_id: &str, app_ids: &[u32]) -> Result<()> {
        self.set_membership(name_or_id, app_ids, true)
    }

    /// Tombstone app ids in a collection.
    pub fn remove(&mut self, name_or_id: &str, app_ids: &[u32]) -> Result<()> {
        self.set_membership(name_or_id, app_ids, false)
    }

    /// Add or tombstone ids; mark pending.
    fn set_membership(&mut self, name_or_id: &str, app_ids: &[u32], member: bool) -> Result<()> {
        let idx = self.resolve_or_create_builtin(name_or_id)?;
        self.ensure_static(idx)?;
        let c = &mut self.collections[idx];
        for &app in app_ids {
            let (keep, drop) = if member {
                (&mut c.added, &mut c.removed)
            } else {
                (&mut c.removed, &mut c.added)
            };
            drop.retain(|&x| x != app);
            if !keep.contains(&app) {
                keep.push(app);
            }
            if !c.pending.contains(&app) {
                c.pending.push(app);
            }
        }
        Ok(())
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

    /// Merge remote; pending ids stay local.
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
                    let legacy = self.format < STORE_FORMAT;
                    match self.collections.iter_mut().find(|c| c.id == incoming.id) {
                        Some(local) => {
                            if legacy {
                                seed_pending(local, &incoming);
                            }
                            let pending = &local.pending;
                            let merge = |remote: &[u32], local: &[u32]| -> Vec<u32> {
                                let mut out: Vec<u32> = remote
                                    .iter()
                                    .copied()
                                    .filter(|a| !pending.contains(a))
                                    .collect();
                                out.extend(local.iter().copied().filter(|a| pending.contains(a)));
                                out
                            };
                            let added = merge(&incoming.added, &local.added);
                            let removed = merge(&incoming.removed, &local.removed);
                            local.added = added;
                            local.removed = removed;
                            local.name = incoming.name;
                            local.filter_spec = incoming.filter_spec;
                        }
                        None => self.collections.push(incoming),
                    }
                }
            }
        }
        self.namespace_version = remote.version;
        self.format = STORE_FORMAT;
    }

    /// Upload entries plus pending-state snapshot.
    fn push_snapshot(&self) -> (Vec<(String, Option<String>)>, PushSnapshot) {
        let entries = self.collections.iter().map(Collection::to_entry).collect();
        let snapshot = self
            .collections
            .iter()
            .map(|c| {
                let state = c.pending.iter().map(|&a| (a, c.contains(a))).collect();
                (c.id.clone(), state)
            })
            .collect();
        (entries, snapshot)
    }

    /// Clear sent pending ids, drop deleted.
    fn finish_push(&mut self, snapshot: &PushSnapshot, deleted: &[String], new_version: u64) {
        for c in &mut self.collections {
            let Some((_, sent)) = snapshot.iter().find(|(id, _)| *id == c.id) else {
                continue;
            };
            let current: Vec<(u32, bool)> = c.pending.iter().map(|&a| (a, c.contains(a))).collect();
            c.pending = current
                .into_iter()
                .filter(|entry| !sent.contains(entry))
                .map(|(a, _)| a)
                .collect();
        }
        // Tombstoned server-side now.
        self.collections
            .retain(|c| !(c.deleted && deleted.contains(&c.id)));
        self.namespace_version = new_version;
        self.format = STORE_FORMAT;
    }
}

/// Legacy: local explicit diffs become pending.
fn seed_pending(local: &mut Collection, remote: &Collection) {
    let explicit: Vec<u32> = local.added.iter().chain(&local.removed).copied().collect();
    for app in explicit {
        let mine = membership(&local.added, &local.removed, app);
        let theirs = membership(&remote.added, &remote.removed, app);
        if mine != theirs && !local.pending.contains(&app) {
            local.pending.push(app);
        }
    }
}

/// Pull, merge and save remote collections.
pub async fn pull(client: &SteamClient) -> Result<CollectionsStore> {
    let remote = client
        .download_collections()
        .await
        .context("failed downloading collections from Steam")?;
    let ((), store) = CollectionsStore::edit(|store| {
        store.apply_remote(remote);
        Ok(())
    })?;
    Ok(store)
}

/// Push every local collection to Steam, then save. On a version conflict the
/// error tells the user to `pull` first.
pub async fn push(client: &SteamClient) -> Result<CollectionsStore> {
    let store = {
        let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        CollectionsStore::load()?
    };
    let (entries, snapshot) = store.push_snapshot();
    let deleted: Vec<String> = store
        .collections
        .iter()
        .filter(|c| c.deleted)
        .map(|c| c.id.clone())
        .collect();

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

    // Reload: keep edits made during upload.
    let ((), store) = CollectionsStore::edit(|store| {
        store.finish_push(&snapshot, &deleted, new_version);
        Ok(())
    })?;
    Ok(store)
}

/// Full sync: `pull` (merge remote in) then `push` (send the merged result).
pub async fn sync(client: &SteamClient) -> Result<CollectionsStore> {
    pull(client).await?;
    push(client).await
}

#[cfg(test)]
#[path = "collections_tests.rs"]
mod tests;
