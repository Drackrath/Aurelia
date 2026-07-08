//! `workshop` command handlers.

use crate::cli::*;
use crate::commands::common::*;

use std::sync::{Arc, RwLock};
use anyhow::{bail, Context, Result};
use aurelia::core::models::DownloadState;
use aurelia::steam_client::SteamClient;

/// Human label for a Workshop entry kind.
pub(crate) fn workshop_kind_label(kind: aurelia::core::models::WorkshopItemKind) -> &'static str {
    match kind {
        aurelia::core::models::WorkshopItemKind::Collection => "collection",
        aurelia::core::models::WorkshopItemKind::Item => "item",
    }
}

/// `workshop browse`: search/browse a game's Workshop to discover items.
pub(crate) async fn cmd_workshop_browse(
    app_id: u32,
    search: Option<String>,
    sort: WorkshopSort,
    count: u32,
    cursor: String,
    tags: Vec<String>,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;
    let page = client
        .query_workshop_files(app_id, search.as_deref(), sort.query_type(), &cursor, count, &tags)
        .await?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "total": page.total,
            "next_cursor": page.next_cursor,
            "items": page.items,
        }));
        return Ok(());
    }

    if page.items.is_empty() {
        cli_println!("No Workshop items found.");
        return Ok(());
    }
    cli_println!("{:>12}  {:>10}  TITLE", "ID", "SIZE");
    for item in &page.items {
        let size = if item.file_size > 0 {
            human_bytes(item.file_size)
        } else {
            "-".to_string()
        };
        cli_println!("{:>12}  {:>10}  {}", item.id, size, item.title);
    }
    cli_println!("\nShowing {} of {} result(s).", page.items.len(), page.total);
    // Offer the next page only when the cursor actually advances.
    if !page.next_cursor.is_empty() && page.next_cursor != cursor {
        cli_println!("Next page: --cursor \"{}\"", page.next_cursor);
    }
    Ok(())
}

/// `workshop info`: show metadata for one or more published files.
pub(crate) async fn cmd_workshop_info(ids: Vec<u64>, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let items = client.fetch_published_file_details(&ids).await?;

    if json {
        cli_println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }
    if items.is_empty() {
        cli_println!("No Workshop items found.");
        return Ok(());
    }
    for item in &items {
        cli_println!("ID      : {}", item.id);
        cli_println!("Title   : {}", item.title);
        cli_println!("App     : {}", item.app_id);
        cli_println!("Type    : {}", workshop_kind_label(item.kind));
        if item.kind == aurelia::core::models::WorkshopItemKind::Collection {
            cli_println!("Items   : {}", item.children.len());
        } else {
            cli_println!("Size    : {}", human_bytes(item.file_size));
            cli_println!("Manifest: {}", item.hcontent_file);
        }
        cli_println!("Updated : {}", format_unix_timestamp(item.time_updated.max(0) as u64));
        cli_println!();
    }
    Ok(())
}

/// `workshop list`: the items you're subscribed to for a game.
pub(crate) async fn cmd_workshop_list(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let ids = client.fetch_subscribed_items(app_id).await?;
    let items = client.fetch_published_file_details(&ids).await?;

    if json {
        cli_println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }
    if items.is_empty() {
        cli_println!("No subscribed Workshop items for app {app_id}.");
        return Ok(());
    }
    cli_println!("{:>12}  {:>12}  TITLE", "ID", "SIZE");
    for item in &items {
        cli_println!("{:>12}  {:>12}  {}", item.id, human_bytes(item.file_size), item.title);
    }
    cli_println!("\n{} subscribed item(s).", items.len());
    Ok(())
}

/// Expand collection ids to leaf item ids unless `no_recurse`.
pub(crate) async fn workshop_resolve_ids(
    client: &SteamClient,
    ids: Vec<u64>,
    no_recurse: bool,
) -> Result<Vec<u64>> {
    if no_recurse {
        Ok(ids)
    } else {
        client.expand_collections(&ids).await
    }
}

/// `workshop install`: download item(s)/collection(s) and register them.
pub(crate) async fn cmd_workshop_install(ids: Vec<u64>, no_recurse: bool, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let leaf_ids = workshop_resolve_ids(&client, ids, no_recurse).await?;
    let items = client.fetch_published_file_details(&leaf_ids).await?;
    if items.is_empty() {
        bail!("no installable Workshop items resolved");
    }

    for item in &items {
        if !json {
            cli_println!("Installing Workshop item {} ({}) ...", item.id, item.title);
        }
        let state = Arc::new(RwLock::new(DownloadState::default()));
        let rx = client
            .install_workshop_item(item, state)
            .await
            .with_context(|| format!("failed to start install for Workshop item {}", item.id))?;
        drive_progress(rx, json).await?;
        if json {
            print_json_line(&serde_json::json!({
                "event": "result",
                "id": item.id,
                "app_id": item.app_id,
                "status": "installed",
            }));
        }
    }
    Ok(())
}

/// `workshop uninstall`: remove installed item(s)/collection(s).
pub(crate) async fn cmd_workshop_uninstall(ids: Vec<u64>, no_recurse: bool, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let leaf_ids = workshop_resolve_ids(&client, ids, no_recurse).await?;
    // GetDetails resolves each item's owning app id (needed to find its files).
    let items = client.fetch_published_file_details(&leaf_ids).await?;

    let mut removed = Vec::new();
    for item in &items {
        client
            .uninstall_workshop_item(item.id, item.app_id)
            .await
            .with_context(|| format!("failed to uninstall Workshop item {}", item.id))?;
        removed.push(item.id);
        if !json {
            cli_println!("Uninstalled Workshop item {} ({}).", item.id, item.title);
        }
    }
    if json {
        print_json(&serde_json::json!({ "uninstalled": removed }));
    } else if removed.is_empty() {
        cli_println!("Nothing to uninstall.");
    }
    Ok(())
}

/// `workshop subscribe`: subscribe (and optionally install) item(s)/collection(s).
pub(crate) async fn cmd_workshop_subscribe(
    ids: Vec<u64>,
    install: bool,
    no_recurse: bool,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;
    let leaf_ids = workshop_resolve_ids(&client, ids, no_recurse).await?;
    let items = client.fetch_published_file_details(&leaf_ids).await?;

    let mut subscribed = Vec::new();
    for item in &items {
        client
            .subscribe_published_file(item.id, item.app_id)
            .await
            .with_context(|| format!("failed to subscribe to Workshop item {}", item.id))?;
        subscribed.push(item.id);
        if !json {
            cli_println!("Subscribed to Workshop item {} ({}).", item.id, item.title);
        }
    }

    if install {
        for item in &items {
            if !json {
                cli_println!("Installing Workshop item {} ...", item.id);
            }
            let state = Arc::new(RwLock::new(DownloadState::default()));
            let rx = client
                .install_workshop_item(item, state)
                .await
                .with_context(|| format!("failed to start install for Workshop item {}", item.id))?;
            drive_progress(rx, json).await?;
        }
    }

    if json {
        print_json(&serde_json::json!({ "subscribed": subscribed, "installed": install }));
    }
    Ok(())
}

/// `workshop unsubscribe`: unsubscribe from item(s)/collection(s).
pub(crate) async fn cmd_workshop_unsubscribe(ids: Vec<u64>, no_recurse: bool, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let leaf_ids = workshop_resolve_ids(&client, ids, no_recurse).await?;
    let items = client.fetch_published_file_details(&leaf_ids).await?;

    let mut unsubscribed = Vec::new();
    for item in &items {
        client
            .unsubscribe_published_file(item.id, item.app_id)
            .await
            .with_context(|| format!("failed to unsubscribe from Workshop item {}", item.id))?;
        unsubscribed.push(item.id);
        if !json {
            cli_println!("Unsubscribed from Workshop item {} ({}).", item.id, item.title);
        }
    }
    if json {
        print_json(&serde_json::json!({ "unsubscribed": unsubscribed }));
    }
    Ok(())
}

/// `workshop status`: installed vs subscribed (with update detection) for a game.
pub(crate) async fn cmd_workshop_status(app_id: u32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let installed = client.read_installed_workshop(app_id).await?;
    // Subscriptions and live manifests need the network; treat them as best-effort
    // so `status` still reports the local install set when offline.
    let subscribed = client.fetch_subscribed_items(app_id).await.unwrap_or_default();

    // Union of installed + subscribed ids, to fetch current manifests for update detection.
    let mut all_ids: Vec<u64> = installed.iter().map(|i| i.id).collect();
    for &s in &subscribed {
        if !all_ids.contains(&s) {
            all_ids.push(s);
        }
    }
    let details = client
        .fetch_published_file_details(&all_ids)
        .await
        .unwrap_or_default();

    // Index details and the installed set once, so resolving each row is a hash
    // lookup rather than a linear scan per id (the loops below run twice over the
    // same data in the JSON and human paths).
    let detail_by_id: std::collections::HashMap<u64, &aurelia::core::models::WorkshopItem> =
        details.iter().map(|d| (d.id, d)).collect();
    let installed_by_id: std::collections::HashMap<u64, &_> =
        installed.iter().map(|i| (i.id, i)).collect();

    // Precompute each row's status once (id, title, installed, subscribed, update),
    // shared by both the JSON and human renderings below.
    struct StatusRow {
        id: u64,
        title: String,
        installed: bool,
        subscribed: bool,
        update: bool,
    }
    let rows: Vec<StatusRow> = all_ids
        .iter()
        .map(|&id| {
            let inst = installed_by_id.get(&id).copied();
            let current_manifest = detail_by_id.get(&id).map(|d| d.hcontent_file);
            let update = match (inst, current_manifest) {
                (Some(i), Some(cur)) => cur != 0 && cur != i.manifest_id,
                _ => false,
            };
            StatusRow {
                id,
                title: detail_by_id.get(&id).map(|d| d.title.clone()).unwrap_or_default(),
                installed: inst.is_some(),
                subscribed: subscribed.contains(&id),
                update,
            }
        })
        .collect();

    if json {
        let arr: Vec<_> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "title": r.title,
                    "installed": r.installed,
                    "subscribed": r.subscribed,
                    "update_available": r.update,
                })
            })
            .collect();
        print_json(&serde_json::json!({ "app_id": app_id, "items": arr }));
        return Ok(());
    }

    if rows.is_empty() {
        cli_println!("No installed or subscribed Workshop items for app {app_id}.");
        return Ok(());
    }
    cli_println!(
        "{:>12}  {:<9}  {:<10}  {:<7}  TITLE",
        "ID", "INSTALLED", "SUBSCRIBED", "UPDATE"
    );
    for r in &rows {
        cli_println!(
            "{:>12}  {:<9}  {:<10}  {:<7}  {}",
            r.id,
            if r.installed { "yes" } else { "no" },
            if r.subscribed { "yes" } else { "no" },
            if r.update { "yes" } else { "-" },
            r.title,
        );
    }
    Ok(())
}

/// `workshop rate`: thumbs-up/down a Workshop item.
pub(crate) async fn cmd_workshop_rate(id: u64, up: bool, json: bool) -> Result<()> {
    let client = authed_client().await?;
    client.vote_workshop_item(id, up).await?;
    if json {
        print_json(&serde_json::json!({
            "id": id,
            "vote": if up { "up" } else { "down" },
            "status": "rated",
        }));
    } else {
        cli_println!(
            "Rated Workshop item {id} {}.",
            if up { "thumbs-up" } else { "thumbs-down" }
        );
    }
    Ok(())
}

/// `workshop comments`: read a page of a Workshop item's comments.
pub(crate) async fn cmd_workshop_comments_read(id: u64, start: i32, count: i32, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let comments = client.workshop_comments(id, start, count).await?;

    if json {
        print_json(&serde_json::json!({ "id": id, "comments": comments }));
        return Ok(());
    }
    if comments.is_empty() {
        cli_println!("No comments on Workshop item {id}.");
        return Ok(());
    }
    for c in &comments {
        cli_println!(
            "[{}] {} (+{})",
            format_unix_timestamp(c.timestamp.max(0) as u64),
            c.author,
            c.upvotes
        );
        cli_println!("  {}", c.text);
    }
    cli_println!("\n{} comment(s).", comments.len());
    Ok(())
}

/// `workshop comment`: post a comment to a Workshop item.
pub(crate) async fn cmd_workshop_comment(id: u64, text: String, json: bool) -> Result<()> {
    let client = authed_client().await?;
    let comment_id = client.post_workshop_comment(id, &text).await?;
    if json {
        print_json(&serde_json::json!({
            "id": id,
            "comment_id": comment_id,
            "status": "posted",
        }));
    } else {
        cli_println!("Posted comment {comment_id} to Workshop item {id}.");
    }
    Ok(())
}
