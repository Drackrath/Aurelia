//! Move, relink and import installs.

use super::*;

pub(crate) async fn cmd_move(
    app_id: u32,
    library: PathBuf,
    restart_steam: bool,
    json: bool,
) -> Result<()> {
    let client = authed_client().await?;

    // Steam rewrites appmanifests and libraryfolders.vdf on exit, so it must not be
    // running while we move things.
    let managed = steam_guard_stop(restart_steam, json)?;
    with_steam_stopped(managed, json, async {
        let rx = client
            .move_install(app_id, library.clone())
            .await
            .with_context(|| format!("failed to start moving app {app_id}"))?;
        drive_progress(rx, json)
            .await
            .with_context(|| format!("failed to move app {app_id}"))
    })
    .await?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "status": "moved",
            "library": library.to_string_lossy(),
            "steam_restarted": managed,
        }));
    } else {
        cli_println!("Moved app {app_id} to {}.", library.display());
    }
    Ok(())
}

/// Steam-guarded relink/import shared body.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the relink/import CLI flags"
)]
pub(super) async fn steam_guarded_relocate<F, Fut>(
    app_id: u32,
    library: PathBuf,
    restart_steam: bool,
    json: bool,
    status: &'static str,
    verb: &'static str,
    human: impl FnOnce(&std::path::Path) -> String,
    op: F,
) -> Result<()>
where
    F: FnOnce(SteamClient, PathBuf) -> Fut,
    Fut: Future<Output = Result<std::path::PathBuf>>,
{
    let client = authed_client().await?;
    let managed = steam_guard_stop(restart_steam, json)?;
    let path = with_steam_stopped(managed, json, async {
        op(client, library.clone())
            .await
            .with_context(|| format!("failed to {verb} app {app_id}"))
    })
    .await?;

    if json {
        print_json(&serde_json::json!({
            "app_id": app_id,
            "status": status,
            "library": library.to_string_lossy(),
            "install_path": path.to_string_lossy(),
            "steam_restarted": managed,
        }));
    } else {
        cli_println!("{}", human(&path));
    }
    Ok(())
}

pub(crate) async fn cmd_relink(
    app_id: u32,
    library: PathBuf,
    restart_steam: bool,
    json: bool,
) -> Result<()> {
    let human = |_: &std::path::Path| format!("Relinked app {app_id} to {}.", library.display());
    steam_guarded_relocate(
        app_id,
        library.clone(),
        restart_steam,
        json,
        "relinked",
        "relink",
        human,
        |client, lib| async move { client.relink_install(app_id, lib).await },
    )
    .await
}

pub(crate) async fn cmd_import(
    app_id: u32,
    library: PathBuf,
    platform: Option<PlatformArg>,
    restart_steam: bool,
    json: bool,
) -> Result<()> {
    // Default to the OS we're running on for the depot/platform match.
    let platform: DepotPlatform =
        platform
            .map(Into::into)
            .unwrap_or(if cfg!(target_os = "windows") {
                DepotPlatform::Windows
            } else {
                DepotPlatform::Linux
            });
    let human = |path: &std::path::Path| format!("Imported app {app_id} from {}.", path.display());
    steam_guarded_relocate(
        app_id,
        library,
        restart_steam,
        json,
        "imported",
        "import",
        human,
        move |client, lib| async move { client.import_install(app_id, lib, platform).await },
    )
    .await
}
