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
        pending: vec![],
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
        pending: vec![],
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
        pending: vec![],
    };
    let (key, value) = c.to_entry();
    assert_eq!(key, "user-collections.uc-x");
    assert!(value.is_none());
}

#[test]
fn merge_follows_remote_and_honors_remote_deletion() {
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
                pending: vec![],
            },
            Collection {
                id: "uc-gone".into(),
                name: "Doomed".into(),
                added: vec![9],
                removed: vec![],
                filter_spec: None,
                deleted: false,
                pending: vec![],
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
        pending: vec![],
    }
    .to_entry();
    let b_entry = Collection {
        id: "uc-b".into(),
        name: "Remote B".into(),
        added: vec![100],
        removed: vec![],
        filter_spec: None,
        deleted: false,
        pending: vec![],
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
    // Nothing pending: uc-a follows remote.
    let a = store.collections.iter().find(|c| c.id == "uc-a").unwrap();
    assert_eq!(a.name, "Remote A");
    assert_eq!(a.added, vec![2, 3]);
    assert_eq!(a.removed, vec![7]);
}

fn coll(id: &str, added: Vec<u32>, removed: Vec<u32>) -> Collection {
    Collection {
        id: id.into(),
        name: id.into(),
        added,
        removed,
        filter_spec: None,
        deleted: false,
        pending: vec![],
    }
}

fn remote_of(version: u64, colls: &[Collection]) -> RemoteNamespace {
    RemoteNamespace {
        version,
        entries: colls.iter().map(Collection::to_entry).collect(),
    }
}

#[test]
fn pending_local_edits_survive_stale_remote() {
    let mut store = CollectionsStore {
        namespace_version: 1,
        collections: vec![coll("uc-a", vec![5], vec![10])],
    };
    // Re-add 10 locally, remove 5.
    store.add("uc-a", &[10]).unwrap();
    store.remove("uc-a", &[5]).unwrap();
    // Remote: 10 removed, 5 added, 7 new.
    store.apply_remote(remote_of(2, &[coll("uc-a", vec![5, 7], vec![10])]));
    let a = store.resolve("uc-a").unwrap();
    assert!(a.contains(10));
    assert!(!a.contains(5));
    // Non-pending id follows remote.
    assert!(a.contains(7));
}

#[test]
fn non_pending_ids_follow_remote() {
    let mut store = CollectionsStore {
        namespace_version: 1,
        collections: vec![coll("uc-a", vec![1, 2], vec![])],
    };
    store.apply_remote(remote_of(2, &[coll("uc-a", vec![2], vec![1])]));
    let a = store.resolve("uc-a").unwrap();
    assert!(!a.contains(1));
    assert!(a.contains(2));
}

#[test]
fn push_keeps_ids_edited_during_upload_pending() {
    let mut store = CollectionsStore::default();
    store.collections.push(coll("uc-a", vec![], vec![]));
    store.add("uc-a", &[1, 2]).unwrap();
    let (_, snapshot) = store.push_snapshot();

    // Concurrent edits after the snapshot.
    let mut fresh = store.clone();
    fresh.remove("uc-a", &[2]).unwrap();
    fresh.add("uc-a", &[3]).unwrap();
    fresh.finish_push(&snapshot, &[], 9);

    let a = fresh.resolve("uc-a").unwrap();
    assert_eq!(fresh.namespace_version, 9);
    assert!(!a.pending.contains(&1));
    assert!(a.pending.contains(&2) && a.pending.contains(&3));
}

#[test]
fn push_drops_only_collections_sent_as_deleted() {
    let mut store = CollectionsStore::default();
    store.collections.push(coll("uc-a", vec![], vec![]));
    store.collections.push(coll("uc-b", vec![], vec![]));
    store.delete("uc-a").unwrap();
    let (_, snapshot) = store.push_snapshot();
    let mut fresh = store.clone();
    fresh.delete("uc-b").unwrap();
    fresh.finish_push(&snapshot, &["uc-a".to_string()], 3);
    assert!(fresh.collections.iter().all(|c| c.id != "uc-a"));
    assert!(fresh.collections.iter().any(|c| c.id == "uc-b"));
}

#[test]
fn builtin_created_on_first_edit_then_renamed_by_pull() {
    let mut store = CollectionsStore::default();
    store.add("favorite", &[440]).unwrap();
    let fav = store.resolve("favorite").unwrap();
    assert!(fav.contains(440) && fav.is_builtin());

    let mut remote = coll("favorite", vec![], vec![]);
    remote.name = "Lieblinge".into();
    store.apply_remote(remote_of(4, &[remote]));
    let fav = store.resolve("favorite").unwrap();
    assert_eq!(fav.name, "Lieblinge");
    assert!(fav.contains(440));
    assert_eq!(store.collections.len(), 1);
    // Unknown names still fail.
    assert!(store.add("nope", &[1]).is_err());
}

#[test]
fn old_store_without_pending_loads() {
    let text = r#"{"namespace_version":3,"collections":[
        {"id":"uc-a","name":"A","added":[1],"removed":[],"deleted":false}]}"#;
    let store: CollectionsStore = serde_json::from_str(text).unwrap();
    assert!(store.collections[0].pending.is_empty());
    let out = serde_json::to_string(&store).unwrap();
    assert!(!out.contains("pending"));
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
                pending: vec![],
            },
            Collection {
                id: "uc-2".into(),
                name: "Dupe".into(),
                added: vec![],
                removed: vec![],
                filter_spec: None,
                deleted: false,
                pending: vec![],
            },
            Collection {
                id: "uc-3".into(),
                name: "Dupe".into(),
                added: vec![],
                removed: vec![],
                filter_spec: None,
                deleted: false,
                pending: vec![],
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
