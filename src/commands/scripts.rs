//! `scripts` command handlers.

use crate::commands::common::*;

use anyhow::{Context, Result};
use aurelia::core::config::load_launcher_config;
use aurelia::core::config::load_library_cache;

/// `scripts dir`: print the resolved launch-script directory.
pub(crate) async fn cmd_scripts_dir(json: bool) -> Result<()> {
    let dir = aurelia::launch::launch_script::script_dir();
    if json {
        print_json(&serde_json::json!({ "script_dir": dir }));
    } else {
        cli_println!("{}", dir.display());
    }
    Ok(())
}

/// `scripts list`: app ids with a launch script (dir-based + config-pinned) and
/// their resolved paths.
pub(crate) async fn cmd_scripts_list(json: bool) -> Result<()> {
    use aurelia::launch::launch_script;
    let cfg = load_launcher_config().await.unwrap_or_default();
    let ids = launch_script::list_script_app_ids(Some(&cfg));

    // Best-effort name resolution from the offline library cache.
    let library = load_library_cache().await.unwrap_or_default();
    let name_of = |id: u32| library.iter().find(|g| g.app_id == id).map(|g| g.name.clone());

    if json {
        let entries: Vec<_> = ids
            .iter()
            .map(|id| {
                serde_json::json!({
                    "app_id": id,
                    "name": name_of(*id),
                    "path": launch_script::resolve(*id, Some(&cfg), None, false),
                })
            })
            .collect();
        print_json(&serde_json::json!({ "scripts": entries }));
        return Ok(());
    }

    if ids.is_empty() {
        cli_println!("No launch scripts. Create one with `aurelia scripts new <app_id>`.");
        return Ok(());
    }
    cli_println!("Launch scripts (dir: {}):", launch_script::script_dir().display());
    for id in ids {
        let name = name_of(id).unwrap_or_else(|| "(unknown)".to_string());
        match launch_script::resolve(id, Some(&cfg), None, false) {
            Some(p) => cli_println!("  {id:<10} {name}\n             {}", p.display()),
            None => cli_println!("  {id:<10} {name}  (unresolved)"),
        }
    }
    Ok(())
}

/// `scripts new`: scaffold a launch script at `<script_dir>/<app_id>.sh|bat`.
pub(crate) async fn cmd_scripts_new(app_id: u32, force: bool, json: bool) -> Result<()> {
    use aurelia::launch::launch_script;
    let dir = launch_script::script_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed creating script dir {}", dir.display()))?;
    let path = launch_script::dir_script_path(app_id);
    if path.exists() && !force {
        anyhow::bail!(
            "a launch script already exists at {} (use --force to overwrite)",
            path.display()
        );
    }
    let name = resolve_game_name(app_id).await.unwrap_or_default();
    let body = launch_script::template(app_id, &name);
    std::fs::write(&path, body).with_context(|| format!("failed writing {}", path.display()))?;

    // Make it directly executable on unix so `exec "$@"`-style wrappers can be run.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms)
            .with_context(|| format!("failed setting executable bit on {}", path.display()))?;
    }

    if json {
        print_json(&serde_json::json!({ "app_id": app_id, "path": path, "status": "created" }));
    } else {
        cli_println!("Created launch script: {}", path.display());
        cli_println!("Edit it, then `aurelia play {app_id}` runs through it.");
    }
    Ok(())
}

/// `scripts show`: print the resolved launch-script path for a game and its contents.
pub(crate) async fn cmd_scripts_show(app_id: u32, json: bool) -> Result<()> {
    use aurelia::launch::launch_script;
    let cfg = load_launcher_config().await.unwrap_or_default();
    let Some(path) = launch_script::resolve(app_id, Some(&cfg), None, false) else {
        if json {
            print_json(&serde_json::json!({ "app_id": app_id, "path": null, "exists": false }));
        } else {
            cli_println!("No launch script for app {app_id}.");
        }
        return Ok(());
    };
    let contents = std::fs::read_to_string(&path).ok();
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "path": path,
            "exists": contents.is_some(),
            "contents": contents,
        }));
    } else {
        cli_println!("Launch script for app {app_id}: {}", path.display());
        match contents {
            Some(c) => {
                cli_println!("");
                cli_print!("{c}");
            }
            None => cli_println!("(file does not exist on disk)"),
        }
    }
    Ok(())
}

/// `scripts remove`: delete the dir-based launch script for a game.
pub(crate) async fn cmd_scripts_remove(app_id: u32, json: bool) -> Result<()> {
    use aurelia::launch::launch_script;
    let removed = launch_script::remove_dir_script(app_id)
        .with_context(|| format!("failed removing launch script for app {app_id}"))?;
    let path = launch_script::dir_script_path(app_id);
    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "path": path,
            "status": if removed { "removed" } else { "not_found" },
        }));
    } else if removed {
        cli_println!("Removed launch script: {}", path.display());
    } else {
        cli_println!("No dir-based launch script for app {app_id} at {}.", path.display());
    }
    Ok(())
}
