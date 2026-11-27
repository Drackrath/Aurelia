//! Stop, pause, list, forget and uninstall.

use super::*;
use crate::app::output::table::Table;

/// `aurelia install stop <app_id>`: signal a running install (tracked in this
/// process's registry) to abort. The download loops poll `abort_signal`.
pub(crate) async fn cmd_install_stop(app_id: u32, json: bool) -> Result<()> {
    signal_install(app_id, false, json)
}

/// `aurelia install pause <app_id>`: stop, but stay `paused`.
pub(crate) async fn cmd_install_pause(app_id: u32, json: bool) -> Result<()> {
    signal_install(app_id, true, json)
}

/// Stop or pause a running or queued install.
pub(super) fn signal_install(app_id: u32, pause: bool, json: bool) -> Result<()> {
    let state = active_installs()
        .lock()
        .ok()
        .and_then(|map| map.get(&app_id).cloned());
    let target = if pause {
        backlog::BacklogState::Paused
    } else {
        backlog::BacklogState::Aborted
    };

    let Some(state) = state else {
        // Not running: just relabel its entry.
        if backlog::set_state(app_id, target)? {
            if json {
                print_result_line(serde_json::json!({
                    "app_id": app_id,
                    "state": target.as_str(),
                }));
            } else {
                cli_println!(
                    "App {app_id} is not downloading; marked {} in the backlog.",
                    target.as_str()
                );
            }
            return Ok(());
        }
        if json {
            print_json(&serde_json::json!({
                "event": "not_found",
                "app_id": app_id,
            }));
        } else {
            cli_eprintln!(
                "no active install for app {app_id} (is the daemon running, and is an install in progress?)"
            );
        }
        return Ok(());
    };

    // Pause first; the abort reads it.
    if let Ok(mut guard) = state.write() {
        guard.is_paused = pause;
        guard.abort_signal.store(true, Ordering::SeqCst);
    }

    if json {
        if pause {
            print_json(&serde_json::json!({
                "event": "pausing",
                "app_id": app_id,
            }));
        } else {
            print_json(&serde_json::json!({
                "event": "stopping",
                "app_id": app_id,
            }));
        }
    } else if pause {
        cli_println!("Pausing install of app {app_id} ...");
    } else {
        cli_println!("Stopping install of app {app_id} ...");
    }
    Ok(())
}

/// One `install list` row.
pub(super) struct ListRow {
    app_id: u32,
    name: String,
    total: u64,
    done: u64,
    status: String,
    downloading: bool,
    state: &'static str,
    entry: Option<backlog::BacklogEntry>,
}

/// `aurelia install list`: running installs, then the backlog.
pub(crate) async fn cmd_install_list(json: bool) -> Result<()> {
    // Snapshot under the lock, then release it before printing.
    let mut rows: Vec<ListRow> = Vec::new();
    if let Ok(map) = active_installs().lock() {
        for state in map.values() {
            if let Ok(s) = state.read() {
                rows.push(ListRow {
                    app_id: s.app_id,
                    name: s.app_name.clone(),
                    total: s.total_bytes,
                    done: s.downloaded_bytes,
                    status: s.status_text.clone(),
                    downloading: s.is_downloading,
                    state: "running",
                    entry: None,
                });
            }
        }
    }
    rows.sort_by_key(|r| r.app_id);
    // A running install's entry is itself.
    let running: Vec<u32> = rows.iter().map(|r| r.app_id).collect();
    for entry in backlog::load()? {
        if running.contains(&entry.app_id) {
            continue;
        }
        rows.push(ListRow {
            app_id: entry.app_id,
            name: entry.name.clone(),
            total: entry.total_bytes,
            done: entry.downloaded_bytes,
            status: entry.reason.clone().unwrap_or_default(),
            downloading: false,
            state: entry.state.as_str(),
            entry: Some(entry),
        });
    }

    if json {
        let arr: Vec<_> = rows
            .iter()
            .map(|r| {
                let mut v = serde_json::json!({
                    "app_id": r.app_id,
                    "name": r.name,
                    "downloaded_bytes": r.done,
                    "total_bytes": r.total,
                    "percent": percent_of(r.done, r.total),
                    "status": r.status,
                    "is_downloading": r.downloading,
                    "state": r.state,
                });
                if let Some(e) = &r.entry {
                    v["platform"] = e.platform.clone().into();
                    v["library"] = e.library.clone().into();
                    v["error_type"] = e.error_type.clone().into();
                    v["added_at"] = e.added_at.into();
                    v["updated_at"] = e.updated_at.into();
                }
                v
            })
            .collect();
        print_json(&serde_json::Value::Array(arr));
        return Ok(());
    }

    if rows.is_empty() {
        cli_println!("No installs in progress or in the backlog.");
        return Ok(());
    }

    let mut table = Table::new()
        .right("AppId")
        .left("State")
        .right("Progress")
        .right("Percent")
        .left("Name");
    for r in &rows {
        let progress = format!("{} / {}", human_bytes(r.done), human_bytes(r.total));
        let label = match (r.name.is_empty(), r.status.is_empty()) {
            (false, true) => r.name.clone(),
            (true, _) => r.status.clone(),
            (false, false) if r.state == "running" => r.name.clone(),
            (false, false) => format!("{} ({})", r.name, r.status),
        };
        table.row([
            r.app_id.to_string(),
            r.state.to_string(),
            progress,
            format!("{:.1}%", percent_of(r.done, r.total)),
            label,
        ]);
    }
    table.print();
    Ok(())
}

/// `aurelia install forget <app_id> [--delete-files]`.
pub(crate) async fn cmd_install_forget(app_id: u32, delete_files: bool, json: bool) -> Result<()> {
    if active_installs()
        .lock()
        .is_ok_and(|m| m.contains_key(&app_id))
    {
        bail!("app {app_id} is downloading; stop it first with `aurelia install stop {app_id}`");
    }
    let Some(entry) = backlog::get(app_id)? else {
        return Err(TypedError::new(
            ErrorKind::NotFound,
            format!("app {app_id} is not in the install backlog"),
        )
        .into());
    };
    let mut deleted: Option<PathBuf> = None;
    if delete_files {
        let Some(dir) = backlog::deletable_dir(&entry) else {
            bail!(
                "won't delete files for app {app_id}: its folder is unknown, shared with a base \
                 game, or belongs to a finished install. Remove it by hand if needed."
            );
        };
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("failed deleting {}", dir.display()))?;
        }
        deleted = Some(dir);
    }
    backlog::remove(app_id)?;
    if json {
        print_result_line(serde_json::json!({
            "app_id": app_id,
            "status": "forgotten",
            "deleted": deleted.as_ref().map(|d| d.to_string_lossy().into_owned()),
        }));
    } else {
        cli_println!("Removed app {app_id} from the install backlog.");
        if let Some(dir) = deleted {
            cli_println!("Deleted {}", dir.display());
        }
    }
    Ok(())
}

pub(crate) async fn cmd_uninstall(app_id: u32, delete_prefix: bool, json: bool) -> Result<()> {
    let client = authed_client().await?;
    client
        .uninstall_game(app_id, delete_prefix)
        .await
        .with_context(|| format!("failed to uninstall app {app_id}"))?;
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "uninstalled": true,
            "deleted_prefix": delete_prefix,
        }));
    } else {
        cli_println!("Uninstalled app {app_id}.");
    }
    Ok(())
}
