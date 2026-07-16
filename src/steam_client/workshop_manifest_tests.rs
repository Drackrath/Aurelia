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
