//! Duplicate install report.

use super::*;

/// Report games installed in more than one Steam library.
///
/// Deliberately read-only: the redundant copy is real game data on a real disk,
/// and which one to keep can depend on things Aurelia can't see (a drive about to
/// be unplugged, a slower disk). So this prints the removal commands rather than
/// running them.
pub(crate) async fn cmd_duplicates(json: bool) -> Result<()> {
    let duplicates = aurelia::library::find_duplicate_installs()
        .await
        .context("could not scan Steam libraries for duplicate installs")?;

    if json {
        let arr: Vec<_> = duplicates
            .iter()
            .map(|d| {
                serde_json::json!({
                    "app_id": d.app_id,
                    "name": d.name,
                    "keep": {
                        "install_path": d.live.install_path.to_string_lossy(),
                        "manifest_path": d.live.manifest_path.to_string_lossy(),
                        "last_updated": d.live.last_updated,
                        "build_id": d.live.build_id,
                    },
                    "remove": d.stale.iter().map(|s| serde_json::json!({
                        "install_path": s.install_path.to_string_lossy(),
                        "manifest_path": s.manifest_path.to_string_lossy(),
                        "last_updated": s.last_updated,
                        "build_id": s.build_id,
                        "size_bytes": aurelia::core::fsutil::dir_size(&s.install_path),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        print_json(&serde_json::json!({ "duplicates": arr }));
        return Ok(());
    }

    if duplicates.is_empty() {
        cli_println!("No game is installed in more than one Steam library.");
        return Ok(());
    }

    cli_println!(
        "{} game(s) installed in more than one Steam library.\n\
         Each duplicate leaves two appmanifests disagreeing about what is installed,\n\
         which is why such a game can report an update on every check.\n",
        duplicates.len()
    );

    let mut reclaimable = 0u64;
    for d in &duplicates {
        let name = d
            .name
            .clone()
            .unwrap_or_else(|| format!("App {}", d.app_id));
        cli_println!("{name}  ({})", d.app_id);
        cli_println!(
            "  keep    {}  (updated {}, build {})",
            d.live.install_path.display(),
            format_unix_timestamp(d.live.last_updated),
            d.live.build_id
        );
        for s in &d.stale {
            let size = aurelia::core::fsutil::dir_size(&s.install_path);
            reclaimable += size;
            cli_println!(
                "  remove  {}  (updated {}, build {}, {})",
                s.install_path.display(),
                format_unix_timestamp(s.last_updated),
                s.build_id,
                human_bytes(size)
            );
        }
        cli_println!("");
    }

    cli_println!(
        "Removing every copy above frees {}.\n",
        human_bytes(reclaimable)
    );
    cli_println!("Close Steam first — it rewrites appmanifests on exit — then run:\n");
    for d in &duplicates {
        for s in &d.stale {
            cli_println!(
                "  rm -rf {}",
                shell_quote(&s.install_path.to_string_lossy())
            );
            cli_println!(
                "  rm -f  {}",
                shell_quote(&s.manifest_path.to_string_lossy())
            );
        }
    }
    cli_println!(
        "\nDelete the appmanifest as well as the directory: on its own the manifest\n\
         still advertises the copy, and discovery would keep finding it."
    );
    Ok(())
}

/// Single-quote a path for a POSIX shell so a copy-pasted `rm` is safe with
/// spaces — which Steam install directories routinely contain.
pub(super) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
