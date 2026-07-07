//! Reader/writer for Steam's per-app Workshop manifest file `appworkshop_<appid>.acf`.
//!
//! The file lives at `<library>/steamapps/workshop/appworkshop_<appid>.acf` and
//! records which Workshop items the Steam client considers installed, along with
//! their sizes, timestamps, and content manifest GIDs.
//!
//! This module is intentionally self-contained (no `use super::*;`) so the
//! integrator can `mod workshop_manifest;` in `steam_client.rs` without any
//! build-order concerns.

use anyhow::{Context, Result};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use crate::core::utils::extract_quoted_values;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Metadata for a single installed Workshop item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstalledWorkshopItem {
    /// The Workshop item's published-file ID (the numeric id in the URL on the
    /// Steam Workshop page).
    pub published_file_id: u64,
    /// The content manifest GID (`hcontent_file`) for the item's depot.
    pub manifest_id: u64,
    /// Size of the item on disk in bytes.
    pub size: u64,
    /// Unix timestamp of the last update to the item.
    pub time_updated: i64,
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Returns `<library_root>/steamapps/workshop/appworkshop_<app_id>.acf`.
pub(crate) fn workshop_manifest_path(library_root: &Path, app_id: u32) -> PathBuf {
    library_root
        .join("steamapps")
        .join("workshop")
        .join(format!("appworkshop_{app_id}.acf"))
}

/// Returns `<library_root>/steamapps/workshop/content/<app_id>/<published_file_id>`.
pub(crate) fn workshop_content_dir(
    library_root: &Path,
    app_id: u32,
    published_file_id: u64,
) -> PathBuf {
    library_root
        .join("steamapps")
        .join("workshop")
        .join("content")
        .join(app_id.to_string())
        .join(published_file_id.to_string())
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

/// Parse an `appworkshop_<appid>.acf` file and return the list of installed
/// Workshop items recorded in it.  Returns an empty `Vec` if the file does
/// not exist (i.e. no items are installed yet).
pub(crate) fn read_workshop_manifest(path: &Path) -> Result<Vec<InstalledWorkshopItem>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed reading workshop manifest {}", path.display()))?;
    Ok(parse_installed_items(&raw))
}

// ---------------------------------------------------------------------------
// Write helpers
// ---------------------------------------------------------------------------

/// Insert or replace the entry for `item.published_file_id` in both
/// `WorkshopItemsInstalled` and `WorkshopItemDetails`, recompute `SizeOnDisk`,
/// and write the file back.  Parent directories are created as needed.
pub(crate) fn upsert_installed_item(
    path: &Path,
    app_id: u32,
    item: InstalledWorkshopItem,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed creating directory {}", parent.display()))?;
    }

    let mut items = read_workshop_manifest(path)?;

    // Replace existing entry with the same id, or append.
    if let Some(existing) = items
        .iter_mut()
        .find(|i| i.published_file_id == item.published_file_id)
    {
        *existing = item;
    } else {
        items.push(item);
    }

    write_workshop_manifest(path, app_id, &items)
}

/// Remove the entry for `published_file_id` from both `WorkshopItemsInstalled`
/// and `WorkshopItemDetails`, recompute `SizeOnDisk`, and write the file back.
/// Succeeds quietly if the file or entry does not exist.
pub(crate) fn remove_installed_item(
    path: &Path,
    app_id: u32,
    published_file_id: u64,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let mut items = read_workshop_manifest(path)?;
    let before = items.len();
    items.retain(|i| i.published_file_id != published_file_id);

    if items.len() == before {
        // Entry was not present — succeed quietly.
        return Ok(());
    }

    write_workshop_manifest(path, app_id, &items)
}

// ---------------------------------------------------------------------------
// Internal: serialise
// ---------------------------------------------------------------------------

fn write_workshop_manifest(path: &Path, app_id: u32, items: &[InstalledWorkshopItem]) -> Result<()> {
    let size_on_disk: u64 = items.iter().map(|i| i.size).sum();

    // Writing directly into the buffer with `write!` avoids allocating a fresh
    // temporary `String` for every item in each block (which the previous
    // `format!` + `push_str` pattern did). The emitted bytes are unchanged.
    let mut content = format!(
        "\"AppWorkshop\"\n{{\n\
         \t\"appid\"\t\t\"{app_id}\"\n\
         \t\"SizeOnDisk\"\t\t\"{size_on_disk}\"\n\
         \t\"NeedsUpdate\"\t\t\"0\"\n\
         \t\"NeedsDownload\"\t\t\"0\"\n"
    );

    // WorkshopItemsInstalled block: size / timeupdated / manifest.
    content.push_str("\t\"WorkshopItemsInstalled\"\n\t{\n");
    for item in items {
        write_item_block(&mut content, item.published_file_id, |buf| {
            let _ = write!(buf, "\t\t\t\"size\"\t\t\"{}\"\n", item.size);
            let _ = write!(buf, "\t\t\t\"timeupdated\"\t\t\"{}\"\n", item.time_updated);
            let _ = write!(buf, "\t\t\t\"manifest\"\t\t\"{}\"\n", item.manifest_id);
        });
    }
    content.push_str("\t}\n");

    // WorkshopItemDetails block: manifest / timeupdated / timetouched.
    content.push_str("\t\"WorkshopItemDetails\"\n\t{\n");
    for item in items {
        write_item_block(&mut content, item.published_file_id, |buf| {
            let _ = write!(buf, "\t\t\t\"manifest\"\t\t\"{}\"\n", item.manifest_id);
            let _ = write!(buf, "\t\t\t\"timeupdated\"\t\t\"{}\"\n", item.time_updated);
            let _ = write!(buf, "\t\t\t\"timetouched\"\t\t\"{}\"\n", item.time_updated);
        });
    }
    content.push_str("\t}\n}\n");

    std::fs::write(path, content)
        .with_context(|| format!("failed writing workshop manifest {}", path.display()))?;
    Ok(())
}

/// Write one `"<id>" { ... }` item sub-block, with the inner field lines
/// supplied by `fields`. Factors out the brace envelope shared by the
/// `WorkshopItemsInstalled` and `WorkshopItemDetails` blocks.
fn write_item_block(buf: &mut String, published_file_id: u64, fields: impl FnOnce(&mut String)) {
    let _ = write!(buf, "\t\t\"{published_file_id}\"\n\t\t{{\n");
    fields(buf);
    buf.push_str("\t\t}\n");
}

// ---------------------------------------------------------------------------
// Internal: parse
// ---------------------------------------------------------------------------

/// Extract all Workshop items from the raw ACF text.
///
/// The parser is a straightforward line-by-line state machine — the same style
/// used by `parse_installed_depots_from_acf` in `manifests.rs`.
fn parse_installed_items(raw: &str) -> Vec<InstalledWorkshopItem> {
    #[derive(Debug, PartialEq)]
    enum Section {
        Other,
        ItemsInstalled,
        ItemDetails,
    }

    let mut items_installed: std::collections::HashMap<u64, ItemInstalled> =
        std::collections::HashMap::new();
    let mut items_details: std::collections::HashMap<u64, ItemDetails> =
        std::collections::HashMap::new();

    let mut section = Section::Other;
    // Depth relative to the section opening brace (0 = not yet inside the block)
    let mut depth: u32 = 0;
    // The id of the item sub-block we are currently inside, if any.
    let mut current_id: Option<u64> = None;

    for line in raw.lines() {
        let trimmed = line.trim();

        // Detect top-level section markers.
        if depth == 0 {
            let quoted = extract_quoted_values(trimmed);
            if quoted.len() == 1 {
                match quoted[0].as_str() {
                    "WorkshopItemsInstalled" => {
                        section = Section::ItemsInstalled;
                        continue;
                    }
                    "WorkshopItemDetails" => {
                        section = Section::ItemDetails;
                        continue;
                    }
                    _ => {}
                }
            }
        }

        if section == Section::Other {
            continue;
        }

        if trimmed == "{" {
            depth += 1;
            continue;
        }

        if trimmed == "}" {
            if depth == 0 {
                // Shouldn't happen in a well-formed file, but guard against it.
                section = Section::Other;
                current_id = None;
                continue;
            }
            depth -= 1;
            if depth == 0 {
                // Closing brace of the top-level section block.
                section = Section::Other;
                current_id = None;
            } else if depth == 1 {
                // Closing brace of an item sub-block.
                current_id = None;
            }
            continue;
        }

        // depth == 1: we are inside the section but not yet inside an item
        // depth == 2: we are inside an item's sub-block
        let quoted = extract_quoted_values(trimmed);
        if depth == 1 && quoted.len() == 1 {
            // Start of an item sub-block: the single quoted value is the item id.
            if let Ok(id) = quoted[0].parse::<u64>() {
                current_id = Some(id);
            }
            continue;
        }

        if depth == 2 {
            let Some(id) = current_id else { continue };
            if quoted.len() >= 2 {
                let key = quoted[0].as_str();
                let val = &quoted[1];
                match section {
                    Section::ItemsInstalled => {
                        items_installed.entry(id).or_default().apply_field(key, val);
                    }
                    Section::ItemDetails => {
                        items_details.entry(id).or_default().apply_field(key, val);
                    }
                    Section::Other => {}
                }
            }
        }
    }

    // Merge: prefer `WorkshopItemsInstalled` as the authoritative source for
    // size/timeupdated; use `WorkshopItemDetails` as a fallback for manifest_id.
    let mut all_ids: Vec<u64> = items_installed.keys().copied().collect();
    all_ids.extend(
        items_details
            .keys()
            .filter(|id| !items_installed.contains_key(id)),
    );
    all_ids.sort_unstable();

    all_ids
        .into_iter()
        .map(|id| {
            let inst = items_installed.get(&id);
            let det = items_details.get(&id);
            InstalledWorkshopItem {
                published_file_id: id,
                manifest_id: inst
                    .map(|i| i.manifest_id)
                    .or_else(|| det.map(|d| d.manifest_id))
                    .unwrap_or(0),
                size: inst.map_or(0, |i| i.size),
                time_updated: inst
                    .map(|i| i.time_updated)
                    .or_else(|| det.map(|d| d.time_updated))
                    .unwrap_or(0),
            }
        })
        .collect()
}

// Temporary storage used during parsing.
#[derive(Default)]
struct ItemInstalled {
    size: u64,
    time_updated: i64,
    manifest_id: u64,
}

impl ItemInstalled {
    /// Apply one `"key" "value"` pair from a `WorkshopItemsInstalled` sub-block.
    /// Unrecognised keys and unparseable values are ignored.
    fn apply_field(&mut self, key: &str, val: &str) {
        match key {
            "size" => self.size = val.parse().unwrap_or(self.size),
            "timeupdated" => self.time_updated = val.parse().unwrap_or(self.time_updated),
            "manifest" => self.manifest_id = val.parse().unwrap_or(self.manifest_id),
            _ => {}
        }
    }
}

#[derive(Default)]
struct ItemDetails {
    manifest_id: u64,
    time_updated: i64,
}

impl ItemDetails {
    /// Apply one `"key" "value"` pair from a `WorkshopItemDetails` sub-block.
    /// Unrecognised keys and unparseable values are ignored.
    fn apply_field(&mut self, key: &str, val: &str) {
        match key {
            "manifest" => self.manifest_id = val.parse().unwrap_or(self.manifest_id),
            "timeupdated" => self.time_updated = val.parse().unwrap_or(self.time_updated),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir() -> PathBuf {
        // Unique per call: tests run on parallel threads of one process and each
        // tears its dir down at the end, so a shared dir would race. A static
        // counter (plus the pid) gives every call its own isolated directory.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir()
            .join(format!("aurelia_workshop_test_{}_{n}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn round_trip_two_items() {
        let dir = temp_dir();
        let path = dir.join("appworkshop_12345.acf");

        let item_a = InstalledWorkshopItem {
            published_file_id: 111_000,
            manifest_id: 9_900_001,
            size: 1_024,
            time_updated: 1_700_000_000,
        };
        let item_b = InstalledWorkshopItem {
            published_file_id: 222_000,
            manifest_id: 9_900_002,
            size: 2_048,
            time_updated: 1_700_000_001,
        };

        upsert_installed_item(&path, 12345, item_a.clone()).unwrap();
        upsert_installed_item(&path, 12345, item_b.clone()).unwrap();

        let items = read_workshop_manifest(&path).unwrap();
        assert_eq!(items.len(), 2, "expected 2 items after two upserts");

        let found_a = items.iter().find(|i| i.published_file_id == 111_000).unwrap();
        assert_eq!(found_a.manifest_id, item_a.manifest_id);
        assert_eq!(found_a.size, item_a.size);
        assert_eq!(found_a.time_updated, item_a.time_updated);

        let found_b = items.iter().find(|i| i.published_file_id == 222_000).unwrap();
        assert_eq!(found_b.manifest_id, item_b.manifest_id);
        assert_eq!(found_b.size, item_b.size);
        assert_eq!(found_b.time_updated, item_b.time_updated);

        // Verify SizeOnDisk in raw text equals sum of sizes.
        let raw = fs::read_to_string(&path).unwrap();
        let expected_sum = item_a.size + item_b.size;
        assert!(
            raw.contains(&format!("\"SizeOnDisk\"\t\t\"{expected_sum}\"")),
            "SizeOnDisk should equal {expected_sum}; raw file:\n{raw}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_replaces_existing_item() {
        let dir = temp_dir();
        let path = dir.join("appworkshop_99999.acf");

        let original = InstalledWorkshopItem {
            published_file_id: 555_000,
            manifest_id: 1_000,
            size: 500,
            time_updated: 100,
        };
        upsert_installed_item(&path, 99999, original).unwrap();

        let updated = InstalledWorkshopItem {
            published_file_id: 555_000,
            manifest_id: 2_000,
            size: 1_000,
            time_updated: 200,
        };
        upsert_installed_item(&path, 99999, updated.clone()).unwrap();

        let items = read_workshop_manifest(&path).unwrap();
        assert_eq!(items.len(), 1, "upsert should replace, not append");
        assert_eq!(items[0].manifest_id, updated.manifest_id);
        assert_eq!(items[0].size, updated.size);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_item() {
        let dir = temp_dir();
        let path = dir.join("appworkshop_777.acf");

        let item_a = InstalledWorkshopItem {
            published_file_id: 10,
            manifest_id: 1,
            size: 100,
            time_updated: 0,
        };
        let item_b = InstalledWorkshopItem {
            published_file_id: 20,
            manifest_id: 2,
            size: 200,
            time_updated: 0,
        };

        upsert_installed_item(&path, 777, item_a).unwrap();
        upsert_installed_item(&path, 777, item_b).unwrap();

        remove_installed_item(&path, 777, 10).unwrap();

        let items = read_workshop_manifest(&path).unwrap();
        assert_eq!(items.len(), 1, "one item should remain after removal");
        assert_eq!(items[0].published_file_id, 20, "item 20 should remain");

        // SizeOnDisk should now reflect only item_b.
        let raw = fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains("\"SizeOnDisk\"\t\t\"200\""),
            "SizeOnDisk should be 200 after removing the 100-byte item; raw:\n{raw}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_nonexistent_item_is_noop() {
        let dir = temp_dir();
        let path = dir.join("appworkshop_888.acf");

        // File doesn't exist — should succeed quietly.
        remove_installed_item(&path, 888, 9999).unwrap();

        // File exists but item is absent — should also succeed quietly.
        let item = InstalledWorkshopItem {
            published_file_id: 1,
            manifest_id: 1,
            size: 1,
            time_updated: 0,
        };
        upsert_installed_item(&path, 888, item).unwrap();
        remove_installed_item(&path, 888, 9999).unwrap(); // 9999 not present

        let items = read_workshop_manifest(&path).unwrap();
        assert_eq!(items.len(), 1, "original item should be untouched");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_missing_file_returns_empty() {
        let path = std::env::temp_dir().join("definitely_does_not_exist_aurora_test.acf");
        let items = read_workshop_manifest(&path).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn path_helpers() {
        let root = Path::new("/some/library");
        let mpath = workshop_manifest_path(root, 42);
        assert_eq!(
            mpath,
            PathBuf::from("/some/library/steamapps/workshop/appworkshop_42.acf")
        );

        let cdir = workshop_content_dir(root, 42, 123456789);
        assert_eq!(
            cdir,
            PathBuf::from("/some/library/steamapps/workshop/content/42/123456789")
        );
    }
}
