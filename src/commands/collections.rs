//! `collections` command handlers.

use crate::commands::common::*;

use anyhow::Result;
use aurelia::library::collections::{self, CollectionsStore};

/// Human-readable kind label for a collection.
pub(crate) fn collection_kind(c: &collections::Collection) -> &'static str {
    if c.is_builtin() {
        "built-in"
    } else if c.is_dynamic() {
        "dynamic"
    } else {
        "static"
    }
}

/// `collections list`: show every collection (offline).
pub(crate) async fn cmd_collections_list(json: bool) -> Result<()> {
    let store = CollectionsStore::load()?;
    let visible: Vec<&collections::Collection> =
        store.collections.iter().filter(|c| !c.deleted).collect();

    if json {
        let items: Vec<serde_json::Value> = visible
            .iter()
            .map(|c| {
                let members = c.added.iter().filter(|a| !c.removed.contains(a)).count();
                serde_json::json!({
                    "id": c.id,
                    "name": c.name,
                    "kind": collection_kind(c),
                    "dynamic": c.is_dynamic(),
                    "count": members,
                })
            })
            .collect();
        print_json(&serde_json::json!({
            "namespace_version": store.namespace_version,
            "collections": items,
        }));
        return Ok(());
    }

    if visible.is_empty() {
        cli_println!("No collections. Create one with `aurelia collections create <name>`,");
        cli_println!("or fetch your Steam collections with `aurelia collections pull`.");
        return Ok(());
    }

    cli_println!("{:<14}  {:<9}  {:>7}  NAME", "ID", "KIND", "GAMES");
    for c in &visible {
        let members = c.added.iter().filter(|a| !c.removed.contains(a)).count();
        let count = if c.is_dynamic() {
            "-".to_string()
        } else {
            members.to_string()
        };
        cli_println!(
            "{:<14}  {:<9}  {:>7}  {}",
            c.id,
            collection_kind(c),
            count,
            c.name
        );
    }
    cli_println!("\n{} collection(s).", visible.len());
    Ok(())
}

/// `collections show <name>`: list a collection's member app ids (offline).
pub(crate) async fn cmd_collections_show(name: String, json: bool) -> Result<()> {
    let store = CollectionsStore::load()?;
    let c = store.resolve(&name)?;
    let members: Vec<u32> = if c.is_dynamic() {
        Vec::new()
    } else {
        c.added.iter().copied().filter(|a| !c.removed.contains(a)).collect()
    };

    if json {
        print_json(&serde_json::json!({
            "id": c.id,
            "name": c.name,
            "kind": collection_kind(c),
            "dynamic": c.is_dynamic(),
            "app_ids": members,
        }));
        return Ok(());
    }

    cli_println!("Collection : {}", c.name);
    cli_println!("Id         : {}", c.id);
    cli_println!("Kind       : {}", collection_kind(c));
    if c.is_dynamic() {
        cli_println!(
            "\nThis is a dynamic (filter-based) collection; Steam computes its members, \
             so they can't be listed offline."
        );
        return Ok(());
    }
    if members.is_empty() {
        cli_println!("\n(no games)");
    } else {
        cli_println!("\nGames ({}):", members.len());
        for app_id in members {
            cli_println!("  {app_id}");
        }
    }
    Ok(())
}

/// `collections create <name>`: make a new static collection (offline).
pub(crate) async fn cmd_collections_create(name: String, json: bool) -> Result<()> {
    let mut store = CollectionsStore::load()?;
    let id = store.create(&name)?;
    if json {
        print_json(&serde_json::json!({ "status": "created", "id": id, "name": name }));
    } else {
        cli_println!("Created collection '{name}' ({id}).");
        cli_println!("Add games with `aurelia collections add \"{name}\" <appid> ...`, then");
        cli_println!("`aurelia collections push` to upload it to your Steam account.");
    }
    Ok(())
}

/// `collections delete <name>`: mark a collection for deletion (offline).
pub(crate) async fn cmd_collections_delete(name: String, json: bool) -> Result<()> {
    let mut store = CollectionsStore::load()?;
    store.delete(&name)?;
    if json {
        print_json(&serde_json::json!({ "status": "deleted", "name": name }));
    } else {
        cli_println!("Marked collection '{name}' for deletion.");
        cli_println!("Run `aurelia collections push` to apply it to your Steam account.");
    }
    Ok(())
}

/// `collections rename <name> <new_name>` (offline).
pub(crate) async fn cmd_collections_rename(name: String, new_name: String, json: bool) -> Result<()> {
    let mut store = CollectionsStore::load()?;
    store.rename(&name, &new_name)?;
    if json {
        print_json(&serde_json::json!({ "status": "renamed", "from": name, "to": new_name }));
    } else {
        cli_println!("Renamed '{name}' to '{new_name}'.");
    }
    Ok(())
}

/// `collections add <name> <appid>...` (offline).
pub(crate) async fn cmd_collections_add(name: String, app_ids: Vec<u32>, json: bool) -> Result<()> {
    let mut store = CollectionsStore::load()?;
    store.add(&name, &app_ids)?;
    if json {
        print_json(&serde_json::json!({ "status": "added", "name": name, "app_ids": app_ids }));
    } else {
        cli_println!("Added {} game(s) to '{name}'.", app_ids.len());
    }
    Ok(())
}

/// `collections remove <name> <appid>...` (offline).
pub(crate) async fn cmd_collections_remove(name: String, app_ids: Vec<u32>, json: bool) -> Result<()> {
    let mut store = CollectionsStore::load()?;
    store.remove(&name, &app_ids)?;
    if json {
        print_json(&serde_json::json!({ "status": "removed", "name": name, "app_ids": app_ids }));
    } else {
        cli_println!("Removed {} game(s) from '{name}'.", app_ids.len());
    }
    Ok(())
}

/// `collections pull`: download and merge Steam's collections into the local store.
pub(crate) async fn cmd_collections_pull(json: bool) -> Result<()> {
    let client = authed_client().await?;
    let mut store = CollectionsStore::load()?;
    collections::pull(&mut store, &client).await?;
    let count = store.collections.iter().filter(|c| !c.deleted).count();
    if json {
        print_json(&serde_json::json!({
            "status": "pulled",
            "namespace_version": store.namespace_version,
            "count": count,
        }));
    } else {
        cli_println!(
            "Pulled collections from Steam. {count} collection(s) locally (version {}).",
            store.namespace_version
        );
    }
    Ok(())
}

/// `collections push`: upload local collections to Steam (mutates the account).
pub(crate) async fn cmd_collections_push(yes: bool, json: bool) -> Result<()> {
    let mut store = CollectionsStore::load()?;
    confirm_cloud_write("upload", store.collections.len(), yes, json)?;
    let client = authed_client().await?;
    collections::push(&mut store, &client).await?;
    let count = store.collections.len();
    if json {
        print_json(&serde_json::json!({
            "status": "pushed",
            "namespace_version": store.namespace_version,
            "count": count,
        }));
    } else {
        cli_println!(
            "Pushed collections to Steam. Now at version {} ({count} collection(s)).",
            store.namespace_version
        );
    }
    Ok(())
}

/// `collections sync`: pull then push (mutates the account).
pub(crate) async fn cmd_collections_sync(yes: bool, json: bool) -> Result<()> {
    let mut store = CollectionsStore::load()?;
    confirm_cloud_write("sync", store.collections.len(), yes, json)?;
    let client = authed_client().await?;
    collections::sync(&mut store, &client).await?;
    let count = store.collections.len();
    if json {
        print_json(&serde_json::json!({
            "status": "synced",
            "namespace_version": store.namespace_version,
            "count": count,
        }));
    } else {
        cli_println!(
            "Synced collections with Steam. Now at version {} ({count} collection(s)).",
            store.namespace_version
        );
    }
    Ok(())
}
