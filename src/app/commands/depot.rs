//! `depots files` and `depots download`.

use crate::app::cli::{DepotManifestArgs, DepotsCommand};
use crate::app::commands::common::*;
use crate::app::output::table::Table;

use anyhow::Result;
use aurelia::library::depot_browser::{
    self as browser, DepotFileDownload, EntryKind, ManifestFileEntry, ManifestRef,
};

fn manifest_ref(args: &DepotManifestArgs) -> ManifestRef {
    ManifestRef::from_options(args.manifest, args.branch.as_deref())
}

pub(crate) async fn cmd_depots_command(command: DepotsCommand, json: bool) -> Result<()> {
    match command {
        DepotsCommand::Files {
            app_id,
            depot_id,
            manifest,
            filter,
        } => {
            cmd_depot_files(
                FilesRequest {
                    app_id,
                    depot_id,
                    manifest: manifest_ref(&manifest),
                    filter,
                },
                json,
            )
            .await
        }
        DepotsCommand::Download {
            app_id,
            depot_id,
            path,
            manifest,
            output,
            force,
        } => {
            let request = DepotFileDownload {
                app_id,
                depot_id,
                manifest: manifest_ref(&manifest),
                path,
                // Already absolute via absolutize_path_args.
                out_dir: output.unwrap_or_else(|| crate::app::output::user_path(".".as_ref())),
                force,
            };
            cmd_depot_download(&request, json).await
        }
    }
}

struct FilesRequest {
    app_id: u32,
    depot_id: u32,
    manifest: ManifestRef,
    filter: Option<String>,
}

/// One text row with markers.
pub(crate) fn entry_cells(entry: &ManifestFileEntry) -> [String; 2] {
    let size = match entry.kind {
        EntryKind::Directory => "<dir>".to_string(),
        EntryKind::Symlink => "<link>".to_string(),
        EntryKind::File => human_bytes(entry.size),
    };
    let suffix = match (entry.kind, &entry.link_target) {
        (EntryKind::Directory, _) => "/".to_string(),
        (EntryKind::Symlink, Some(target)) => format!(" -> {target}"),
        (EntryKind::File, _) if entry.executable => "*".to_string(),
        _ => String::new(),
    };
    [size, format!("{}{suffix}", entry.filename)]
}

/// `depots files --json` payload.
pub(crate) fn files_json(
    app_id: u32,
    depot_id: u32,
    manifest_id: u64,
    files: &[ManifestFileEntry],
) -> serde_json::Value {
    let rows: Vec<_> = files
        .iter()
        .map(|f| {
            serde_json::json!({
                "path": f.filename,
                "kind": f.kind,
                "size": f.size,
                "executable": f.executable,
                "link_target": f.link_target,
                "sha1": f.sha1,
                "chunks": f.chunks,
            })
        })
        .collect();
    serde_json::json!({
        "app_id": app_id,
        "depot_id": depot_id,
        "manifest_id": manifest_id,
        "total_size": browser::total_size(files),
        "files": rows,
    })
}

async fn cmd_depot_files(request: FilesRequest, json: bool) -> Result<()> {
    let FilesRequest {
        app_id,
        depot_id,
        manifest,
        filter,
    } = request;
    let client = authed_client().await?;
    let (manifest_id, mut files) = client
        .depot_manifest_files(app_id, depot_id, &manifest)
        .await?;
    if let Some(pattern) = &filter {
        files.retain(|f| browser::glob_match(pattern, &f.filename));
    }

    if json {
        print_json(&files_json(app_id, depot_id, manifest_id, &files));
        return Ok(());
    }

    cli_println!("App {app_id}, depot {depot_id}, manifest {manifest_id}");
    if files.is_empty() {
        cli_println!(
            "{}",
            if filter.is_some() {
                "No entries match the filter."
            } else {
                "The manifest is empty."
            }
        );
        return Ok(());
    }
    let mut table = Table::new().right("Size").left("Path");
    for f in &files {
        table.row(entry_cells(f));
    }
    table.print();
    let count = files.iter().filter(|f| f.kind == EntryKind::File).count();
    cli_println!(
        "\n{count} files, {}",
        human_bytes(browser::total_size(&files))
    );
    Ok(())
}

async fn cmd_depot_download(request: &DepotFileDownload, json: bool) -> Result<()> {
    let client = authed_client().await?;
    if !json {
        cli_eprintln!(
            "Downloading {} from depot {}...",
            request.path,
            request.depot_id
        );
    }
    let done = client.download_depot_file(request).await?;

    if json {
        print_json(&serde_json::json!({
            "app_id": request.app_id,
            "depot_id": request.depot_id,
            "manifest_id": done.manifest_id,
            "path": done.path,
            "size": done.size,
        }));
        return Ok(());
    }
    cli_println!(
        "Saved {} ({}, manifest {}).",
        done.path.display(),
        human_bytes(done.size),
        done.manifest_id
    );
    Ok(())
}

#[cfg(test)]
mod tests;
