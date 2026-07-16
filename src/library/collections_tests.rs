use super::*;
use serde_json::json;

#[test]
fn static_entry_round_trips() {
    let c = Collection {
        id: "uc-0001".into(),
        name: "RPGs".into(),
        added: vec![10, 20, 30],
        removed: vec![20],
        filter_spec: None,
        deleted: false,
    };
    let (key, value) = c.to_entry();
    assert_eq!(key, "user-collections.uc-0001");
    let parsed = Collection::from_entry(&key, &value.unwrap()).unwrap();
    assert_eq!(parsed, c);
    // Membership: 10 in, 20 removed, 30 in.
    assert!(parsed.contains(10));
    assert!(!parsed.contains(20));
    assert!(parsed.contains(30));
}

#[test]
fn dynamic_entry_preserves_filter_spec() {
    let filter = json!({
        "nFormatVersion": 2,
        "filterGroups": [{ "rgOptions": [492], "bAcceptUnion": false }],
    });
    let c = Collection {
        id: "uc-dyn".into(),
        name: "Free Games".into(),
        added: vec![],
        removed: vec![],
        filter_spec: Some(filter.clone()),
        deleted: false,
    };
    let (key, value) = c.to_entry();
    let value = value.unwrap();
    // The opaque filter must survive verbatim.
    assert!(value.contains("filterGroups"));
    let parsed = Collection::from_entry(&key, &value).unwrap();
    assert!(parsed.is_dynamic());
    assert_eq!(parsed.filter_spec.as_ref().unwrap(), &filter);
    assert_eq!(parsed, c);
}

#[test]
fn deleted_collection_becomes_tombstone() {
    let c = Collection {
        id: "uc-x".into(),
        name: "Gone".into(),
        added: vec![1],
        removed: vec![],
        filter_spec: None,
        deleted: true,
    };
    let (key, value) = c.to_entry();
    assert_eq!(key, "user-collections.uc-x");
    assert!(value.is_none());
}

#[test]
fn merge_unions_and_honors_remote_deletion() {
    let mut store = CollectionsStore {
        namespace_version: 5,
        collections: vec![
            Collection {
                id: "uc-a".into(),
                name: "Local A".into(),
                added: vec![1, 2],
                removed: vec![],
                filter_spec: None,
                deleted: false,
            },
            Collection {
                id: "uc-gone".into(),
                name: "Doomed".into(),
                added: vec![9],
                removed: vec![],
                filter_spec: None,
                deleted: false,
            },
        ],
    };

    // Remote: adds appid 3 to uc-a and renames it, brings a brand-new uc-b,
    // and tombstones uc-gone.
    let a_entry = Collection {
        id: "uc-a".into(),
        name: "Remote A".into(),
        added: vec![2, 3],
        removed: vec![7],
        filter_spec: None,
        deleted: false,
    }
    .to_entry();
    let b_entry = Collection {
        id: "uc-b".into(),
        name: "Remote B".into(),
        added: vec![100],
        removed: vec![],
        filter_spec: None,
        deleted: false,
    }
    .to_entry();

    let remote = RemoteNamespace {
        version: 11,
        entries: vec![
            (a_entry.0, a_entry.1),
            (b_entry.0, b_entry.1),
            ("user-collections.uc-gone".into(), None),
        ],
    };
    store.apply_remote(remote);

    assert_eq!(store.namespace_version, 11);
    // uc-gone dropped.
    assert!(store.collections.iter().all(|c| c.id != "uc-gone"));
    // uc-b added.
    assert!(store.collections.iter().any(|c| c.id == "uc-b"));
    // uc-a: union of added {1,2}∪{2,3} = {1,2,3}; removed {7}; remote name.
    let a = store.collections.iter().find(|c| c.id == "uc-a").unwrap();
    assert_eq!(a.name, "Remote A");
    assert!(a.added.contains(&1) && a.added.contains(&2) && a.added.contains(&3));
    assert!(a.removed.contains(&7));
}

#[test]
fn resolve_by_name_id_and_ambiguity() {
    let store = CollectionsStore {
        namespace_version: 0,
        collections: vec![
            Collection {
                id: "uc-1".into(),
                name: "Shooters".into(),
                added: vec![],
                removed: vec![],
                filter_spec: None,
                deleted: false,
            },
            Collection {
                id: "uc-2".into(),
                name: "Dupe".into(),
                added: vec![],
                removed: vec![],
                filter_spec: None,
                deleted: false,
            },
            Collection {
                id: "uc-3".into(),
                name: "Dupe".into(),
                added: vec![],
                removed: vec![],
                filter_spec: None,
                deleted: false,
            },
        ],
    };
    // Case-insensitive name.
    assert_eq!(store.resolve("shooters").unwrap().id, "uc-1");
    // Exact id.
    assert_eq!(store.resolve("uc-2").unwrap().id, "uc-2");
    // Unknown.
    assert!(store.resolve("nope").is_err());
    // Ambiguous name.
    assert!(store.resolve("Dupe").is_err());
}
