pub mod pipeline;
pub mod stages;
pub mod validators;
pub mod dll_provider_resolver;
pub mod fixups;
pub mod launch_script;

#[cfg(test)]
mod verification_tests;

use std::path::{Path, PathBuf};
use std::process::Command;
use anyhow::{Result, Context, anyhow};
use crate::core::config::{config_dir, LauncherConfig};
use crate::steam_client::SteamClient;
use crate::core::utils::MasterSteamConfig;

/// Ensure Steam is installed into the master Windows prefix, then start it.
///
/// Two distinct phases with very different process semantics:
///
/// 1. If `steam.exe` is absent, run `SteamSetup.exe` **synchronously** under a bare
///    wine and verify it actually produced `steam.exe`. The installer is a bounded
///    job, so we wait on it and surface its exit code.
/// 2. Launch `steam.exe` **detached** — that is the long-running background Steam
///    client and must not block the caller.
pub async fn install_master_steam(config: &LauncherConfig) -> Result<()> {
    let base_dir = config_dir()?;
    let steam_cfg = crate::core::utils::get_master_steam_config();

    // Resolve the runner FIRST: a misconfigured runner must fail before we spend a
    // download on an installer we have no way to execute.
    let runner_name = config.steam_runtime_runner.to_string_lossy();
    let library_root = PathBuf::from(&config.steam_library_path);
    let wine = crate::core::utils::resolve_steam_runtime_wine(&runner_name, &library_root)?;

    tracing::info!("Unified Master Steam resolution:");
    tracing::info!("  - Root Dir: {}", steam_cfg.root_dir.display());
    tracing::info!("  - Wine Prefix: {}", steam_cfg.wine_prefix.display());
    tracing::info!("  - Layout Kind: {}", steam_cfg.layout_kind);
    tracing::info!("  - Wine Binary: {}", wine.display());

    let steam_exe = match steam_cfg.steam_exe.clone() {
        Some(exe) => {
            tracing::info!("  - Steam Exe: {} (already installed)", exe.display());
            exe
        }
        None => {
            tracing::info!("  - Steam Exe: NOT FOUND (running installer)");
            run_steam_installer(&wine, &steam_cfg, &base_dir).await?
        }
    };

    // Bare-wine prefixes miss the runner's dxvk/vkd3d PE libs the Steam CEF UI
    // needs (see ensure_steam_runtime_prefix_libs) — sync them before launching.
    crate::core::utils::ensure_steam_runtime_prefix_libs(&wine, &steam_cfg.wine_prefix);

    // Same guard the game-launch path uses: a second `install` while a client is
    // already up must not double-spawn Steam (it only pops a "Steam is already
    // running" dialog in the prefix).
    if SteamClient::is_steam_running_in_prefix(&steam_cfg.wine_prefix) {
        tracing::info!("Steam already running in the master prefix — skipping spawn");
        return Ok(());
    }

    launch_master_steam(&wine, &steam_exe, &steam_cfg, &base_dir)
}

/// Download (if needed) and run `SteamSetup.exe` to completion under `wine`.
/// Returns the path to the installed `steam.exe`.
async fn run_steam_installer(
    wine: &Path,
    steam_cfg: &MasterSteamConfig,
    base_dir: &Path,
) -> Result<PathBuf> {
    let runtimes_dir = base_dir.join("runtimes");
    std::fs::create_dir_all(&runtimes_dir)?;
    let setup_exe = runtimes_dir.join("SteamSetup.exe");
    ensure_steam_setup(&setup_exe).await?;

    // Create the WINEPREFIX ourselves before invoking the installer. On a fresh
    // install the `pfx` layout points WINEPREFIX at `root_dir/pfx`, whose parent
    // does not exist yet; SteamSetup.exe then ran against a missing prefix and users
    // had to `mkdir -p .../master_steam_prefix/pfx` by hand first (issue #2).
    std::fs::create_dir_all(&steam_cfg.wine_prefix).with_context(|| {
        format!(
            "Failed to create master Steam prefix at {}",
            steam_cfg.wine_prefix.display()
        )
    })?;

    let mut cmd = Command::new(wine);
    cmd.arg(&setup_exe);
    // `/S` is the NSIS silent-install switch. Without it SteamSetup.exe opens its
    // interactive wizard and waits for a human, so an `install` on a headless or
    // unattended machine simply never completes.
    cmd.arg("/S");
    // Keep the installer's Wine `fixme:`/NSIS chatter off the terminal by default;
    // apply_install_diagnostics redirects it to a log file when diagnostics are on.
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    apply_master_steam_env(&mut cmd, steam_cfg, base_dir)?;
    apply_install_diagnostics(&mut cmd, base_dir);

    tracing::info!("Running Steam installer: {:?}", cmd);

    // Wait for the installer: it is a bounded job, unlike background Steam. The old
    // code spawned it detached and dropped the child, so a wine that died on startup
    // still reported "install started" and left the caller with no diagnostics.
    let status = tokio::process::Command::from(cmd)
        .status()
        .await
        .with_context(|| format!("Failed to run Steam installer under {}", wine.display()))?;

    if !status.success() {
        return Err(anyhow!(
            "SteamSetup.exe failed under {} (exit status: {status}). \
             Re-run with AURELIA_DIAGNOSE_INSTALL=1 to capture a wine debug log.",
            wine.display()
        ));
    }

    // Re-probe rather than trusting the exit code: NSIS happily returns 0 when it
    // silently declines to install anything.
    crate::core::utils::get_master_steam_config()
        .steam_exe
        .ok_or_else(|| {
            anyhow!(
                "SteamSetup.exe exited successfully but no steam.exe appeared under {}. \
                 Re-run with AURELIA_DIAGNOSE_INSTALL=1 to capture a wine debug log.",
                steam_cfg.wine_prefix.display()
            )
        })
}

/// Start the background Steam client detached. It is long-running by design, so the
/// caller must not wait on it.
fn launch_master_steam(
    wine: &Path,
    steam_exe: &Path,
    steam_cfg: &MasterSteamConfig,
    base_dir: &Path,
) -> Result<()> {
    // Refresh steam.cfg on every launch, not just at install: Steam's self-update
    // rewrites (or drops) the file, losing `BootStrapperForceSelfUpdate=disable` —
    // the guard that keeps the bootstrapper from wedging the client under Wine.
    if let Some(steam_dir) = steam_exe.parent() {
        SteamClient::write_headless_steam_cfg(steam_dir);
    }

    let mut cmd = Command::new(wine);
    cmd.arg(steam_exe);
    // Steam *client* flags tuned for running under Wine. Steam's CEF UI
    // (steamwebhelper) is fragile under Wine: with GPU/sandbox left on it flashes
    // the "Steam is updating" bootstrapper and then vanishes instead of showing the
    // login window. These are the same flags the in-Wine background Steam uses for
    // game launches, minus `-silent` (here we WANT the UI so the user can sign in).
    cmd.arg("-tcp");
    cmd.arg("-noreactlogin");
    cmd.arg("-cef-disable-gpu");
    cmd.arg("-cef-disable-sandbox");
    cmd.arg("-cef-disable-gpu-compositing");
    // Silence the background client's very chatty stdout/stderr (Wine `fixme:` spam
    // + Steam bootstrapper logging) so it doesn't clutter the terminal — this is a
    // detached GUI process, its console output is noise. `apply_install_diagnostics`
    // redirects both to a log file instead when AURELIA_DIAGNOSE_INSTALL=1.
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    apply_master_steam_env(&mut cmd, steam_cfg, base_dir)?;
    apply_install_diagnostics(&mut cmd, base_dir);

    tracing::info!("Launching Master Steam: {:?}", cmd);
    let _child = cmd.spawn().context("Failed to spawn master steam process")?;
    Ok(())
}

/// Environment shared by the installer and the background Steam client.
fn apply_master_steam_env(
    cmd: &mut Command,
    steam_cfg: &MasterSteamConfig,
    base_dir: &Path,
) -> Result<()> {
    cmd.env("WINEPREFIX", &steam_cfg.wine_prefix);
    cmd.env("STEAM_COMPAT_DATA_PATH", &steam_cfg.root_dir);
    cmd.env("WINEPATH", "C:\\Program Files (x86)\\Steam");

    let fake_env = crate::core::utils::setup_fake_steam_trap(base_dir)?;
    cmd.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &fake_env);
    cmd.env("WINEDLLOVERRIDES", "vstdlib_s=n;tier0_s=n;steamclient=n;steamclient64=n;steam_api=n;steam_api64=n;lsteamclient=");

    // Forward the display environment so the Steam client can actually draw a window.
    // XAUTHORITY is essential on X servers that use cookie authentication (most modern
    // desktops place the cookie under $XDG_RUNTIME_DIR, not ~/.Xauthority): without it
    // Wine's winex11 driver can't authenticate to the X server and Steam runs
    // *invisibly* even though DISPLAY is set — so `steam-runtime login` shows no window.
    for var in ["DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "XAUTHORITY"] {
        if let Ok(value) = std::env::var(var) {
            cmd.env(var, value);
        }
    }
    Ok(())
}

/// Opt-in Steam-runtime install diagnostics.
///
/// When `AURELIA_DIAGNOSE_INSTALL=1` the install/repair flow runs with verbose
/// WINEDEBUG channels (setupapi/file/module) that surface the file-copy and
/// DLL-registration failures typical of a broken Steam install, and its stdout/stderr
/// are captured to a timestamped log file under `config_dir()/logs`, the same root the
/// launch pipeline uses. This path is ISOLATED to master-Steam install/repair and never
/// touches normal game launches. With the var unset, behavior is unchanged.
fn apply_install_diagnostics(cmd: &mut Command, base_dir: &Path) {
    if std::env::var("AURELIA_DIAGNOSE_INSTALL").as_deref() != Ok("1") {
        return;
    }
    let logs_dir = base_dir.join("logs");
    if let Err(e) = std::fs::create_dir_all(&logs_dir) {
        tracing::warn!("AURELIA_DIAGNOSE_INSTALL set but could not create log dir {}: {}", logs_dir.display(), e);
        return;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let log_path = logs_dir.join(format!("steam_runtime_install_{stamp}.log"));
    match std::fs::File::create(&log_path) {
        Ok(file) => {
            cmd.env("WINEDEBUG", "+setupapi,+file,+module");
            if let Ok(err_file) = file.try_clone() {
                cmd.stderr(std::process::Stdio::from(err_file));
            }
            cmd.stdout(std::process::Stdio::from(file));
            tracing::info!(
                "AURELIA_DIAGNOSE_INSTALL=1: capturing Steam-runtime install diagnostics to {}",
                log_path.display()
            );
        }
        Err(e) => tracing::warn!(
            "AURELIA_DIAGNOSE_INSTALL set but could not create diagnostic log {}: {}",
            log_path.display(), e
        ),
    }
}

/// The Steam-dir subdirectories that hold user state a repair must never lose:
/// `config` (logins / client settings), `userdata` (per-account data) and
/// `steamapps` (games installed *through* the in-Wine Steam — the only copy of
/// Family-Shared titles Aurelia cannot download itself).
const PRESERVED_STEAM_DATA_DIRS: [&str; 3] = ["userdata", "config", "steamapps"];

/// `path` with `suffix` appended to its final component (`…/pfx` → `…/pfx.bak`).
fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// The in-prefix Steam directory, if one exists (prefers the one holding steam.exe).
fn find_master_steam_dir(steam_cfg: &MasterSteamConfig) -> Option<PathBuf> {
    if let Some(exe) = &steam_cfg.steam_exe {
        return exe.parent().map(Path::to_path_buf);
    }
    // A broken install can lack steam.exe while the data dirs survive — probe the
    // same candidate locations find_steam_exe_in_prefix uses.
    [
        "drive_c/Program Files (x86)/Steam",
        "drive_c/Program Files/Steam",
    ]
    .into_iter()
    .map(|rel| steam_cfg.wine_prefix.join(rel))
    .find(|dir| dir.is_dir())
}

/// Move the user-data dirs ([`PRESERVED_STEAM_DATA_DIRS`]) out of `steam_dir` into
/// `holding_dir` before a repair touches the Steam install. Returns which dirs were
/// actually moved (missing ones are skipped).
///
/// The holding folder must live OUTSIDE the Steam dir and the data must stay there
/// until SteamSetup.exe has *exited*: NSIS refuses a non-empty destination
/// ("destination folder should be empty"), so restoring early aborts the reinstall.
pub fn preserve_steam_data_dirs(steam_dir: &Path, holding_dir: &Path) -> Result<Vec<&'static str>> {
    let mut preserved = Vec::new();
    for name in PRESERVED_STEAM_DATA_DIRS {
        let src = steam_dir.join(name);
        if !src.is_dir() {
            continue;
        }
        std::fs::create_dir_all(holding_dir).with_context(|| {
            format!("failed creating repair holding dir {}", holding_dir.display())
        })?;
        let dst = holding_dir.join(name);
        if dst.exists() {
            // Stale leftover from an interrupted repair; the live Steam dir is
            // authoritative when both exist.
            std::fs::remove_dir_all(&dst)
                .with_context(|| format!("failed removing stale {}", dst.display()))?;
        }
        std::fs::rename(&src, &dst).with_context(|| {
            format!("failed moving {} to {}", src.display(), dst.display())
        })?;
        preserved.push(name);
    }
    Ok(preserved)
}

/// Move previously preserved data dirs from `holding_dir` back into `steam_dir`
/// (created if needed). A dir the fresh install re-created is replaced — the
/// preserved copy wins, the fresh one only holds installer defaults. Returns which
/// dirs were restored; removes the holding dir once emptied.
pub fn restore_steam_data_dirs(holding_dir: &Path, steam_dir: &Path) -> Result<Vec<&'static str>> {
    let mut restored = Vec::new();
    if !holding_dir.is_dir() {
        return Ok(restored);
    }
    std::fs::create_dir_all(steam_dir)
        .with_context(|| format!("failed creating {}", steam_dir.display()))?;
    for name in PRESERVED_STEAM_DATA_DIRS {
        let src = holding_dir.join(name);
        if !src.is_dir() {
            continue;
        }
        let dst = steam_dir.join(name);
        if dst.exists() {
            std::fs::remove_dir_all(&dst).with_context(|| {
                format!("failed removing freshly installed {}", dst.display())
            })?;
        }
        std::fs::rename(&src, &dst).with_context(|| {
            format!("failed restoring {} to {}", src.display(), dst.display())
        })?;
        restored.push(name);
    }
    // Best-effort: only succeeds once the holding dir is empty.
    let _ = std::fs::remove_dir(holding_dir);
    Ok(restored)
}

/// Repair the master Windows-Steam prefix without losing user data: stop anything
/// holding it, move `userdata`/`config`/`steamapps` into a holding folder, snapshot
/// the rest of the prefix (retaining a single `.bak` safety net), re-run the
/// installer into a fresh prefix, then restore the preserved dirs into it — so
/// logins and games installed inside the in-Wine Steam survive the repair.
///
/// Like [`install_master_steam`], this needs a configured `steam_runtime_runner`
/// to drive the installer under a bare wine. The runner is validated up front so
/// the destructive backup step never runs when the reinstall would fail anyway.
pub async fn repair_master_steam(config: &LauncherConfig) -> Result<()> {
    if config.steam_runtime_runner.as_os_str().is_empty() {
        return Err(anyhow!(
            "{}",
            crate::core::utils::steam_runtime_runner_unset_msg("repairing")
        ));
    }

    let base_dir = config_dir()?;
    let steam_cfg = crate::core::utils::get_master_steam_config();

    // Resolve the runner FIRST — see the doc comment above.
    let runner_name = config.steam_runtime_runner.to_string_lossy();
    let library_root = PathBuf::from(&config.steam_library_path);
    let wine = crate::core::utils::resolve_steam_runtime_wine(&runner_name, &library_root)?;

    // 1. Kill any master-Steam / game processes still holding the prefix so the
    //    directory can be moved safely. Reuse the existing prefix-scoped killers
    //    rather than inventing a new mechanism. `kill_steam_in_prefix` is
    //    cross-platform (a no-op on Windows); the broader wine sweep is unix-only.
    tracing::info!(
        "Repair: stopping any processes holding the master prefix {}",
        steam_cfg.wine_prefix.display()
    );
    SteamClient::kill_steam_in_prefix(&steam_cfg.wine_prefix);
    #[cfg(unix)]
    SteamClient::kill_wine_processes_in_prefix(&steam_cfg.wine_prefix, true);
    // Give the wineserver a moment to release open file handles before the moves.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // 2. Move the user data out of the Steam dir into a holding folder next to the
    //    prefix (outside it, so the `.bak` rename below can't take the data along).
    let steam_dir = find_master_steam_dir(&steam_cfg);
    let holding = sibling_with_suffix(&steam_cfg.wine_prefix, ".repair-data");
    let preserved = match &steam_dir {
        Some(dir) => {
            let preserved = preserve_steam_data_dirs(dir, &holding)?;
            if !preserved.is_empty() {
                tracing::info!(
                    "Repair: preserved {} into {}",
                    preserved.join(", "),
                    holding.display()
                );
            }
            preserved
        }
        None => Vec::new(),
    };

    // 3 + 4, jointly fallible so a failure in either step can hand the preserved
    // data back instead of stranding it in the holding folder.
    let install_result = async {
        // 3. Snapshot the rest of the prefix, retaining only ONE backup. This rename
        //    is also what removes the broken client files from the live tree: with
        //    the data dirs held aside, everything the rename takes away is
        //    client/wine state the installer rebuilds — kept recoverable in the
        //    `.bak` instead of deleted (`steam-runtime restore` swaps it back; the
        //    next repair rotates it away).
        if steam_cfg.wine_prefix.exists() {
            let bak = sibling_with_suffix(&steam_cfg.wine_prefix, ".bak");
            if bak.exists() {
                tracing::info!("Repair: removing previous backup {}", bak.display());
                std::fs::remove_dir_all(&bak).with_context(|| {
                    format!("failed removing previous backup {}", bak.display())
                })?;
            }
            tracing::info!(
                "Repair: backing up {} -> {}",
                steam_cfg.wine_prefix.display(),
                bak.display()
            );
            std::fs::rename(&steam_cfg.wine_prefix, &bak)
                .with_context(|| format!("failed backing up master prefix to {}", bak.display()))?;
        } else {
            tracing::info!(
                "Repair: no existing master prefix at {} — nothing to back up",
                steam_cfg.wine_prefix.display()
            );
        }

        // 4. Re-run the installer into the now-clean prefix. NOT install_master_steam:
        //    that would launch Steam before the data is back in place.
        run_steam_installer(&wine, &steam_cfg, &base_dir).await
    }
    .await;

    let steam_exe = match install_result {
        Ok(exe) => exe,
        Err(e) => {
            // Return the preserved data to wherever the old Steam dir now lives —
            // inside the `.bak` if the snapshot happened (so `steam-runtime restore`
            // recovers the complete old prefix), else the still-live prefix.
            if !preserved.is_empty() {
                if let Some(dir) = &steam_dir {
                    let bak = sibling_with_suffix(&steam_cfg.wine_prefix, ".bak");
                    let dest = if bak.exists() {
                        dir.strip_prefix(&steam_cfg.wine_prefix)
                            .map(|rel| bak.join(rel))
                            .unwrap_or_else(|_| dir.clone())
                    } else {
                        dir.clone()
                    };
                    match restore_steam_data_dirs(&holding, &dest) {
                        Ok(_) => tracing::warn!(
                            "Repair failed; preserved data returned to {}",
                            dest.display()
                        ),
                        Err(re) => tracing::error!(
                            "Repair failed AND restoring preserved data failed ({re}); \
                             your userdata/config/steamapps remain in {}",
                            holding.display()
                        ),
                    }
                }
            }
            return Err(e);
        }
    };

    // 5. Restore the preserved data into the fresh install — only now that the
    //    installer has exited (see preserve_steam_data_dirs on why not earlier).
    if let Some(new_steam_dir) = steam_exe.parent() {
        let restored = restore_steam_data_dirs(&holding, new_steam_dir)?;
        if !restored.is_empty() {
            tracing::info!(
                "Repair: restored {} into {}",
                restored.join(", "),
                new_steam_dir.display()
            );
        }
    }

    crate::core::utils::ensure_steam_runtime_prefix_libs(&wine, &steam_cfg.wine_prefix);
    launch_master_steam(&wine, &steam_exe, &steam_cfg, &base_dir)
}

/// Restore the master prefix from the `.bak` safety net a previous
/// [`repair_master_steam`] left, by **swapping** the current prefix with the backup
/// (running `restore` again swaps back, so nothing is ever deleted). Stops anything
/// holding the prefix first. Returns `true` if a current prefix existed and was
/// swapped, `false` if the backup was simply moved into place.
pub async fn restore_master_steam() -> Result<bool> {
    let steam_cfg = crate::core::utils::get_master_steam_config();

    let mut target = steam_cfg.wine_prefix.clone();
    let mut bak = sibling_with_suffix(&target, ".bak");
    if !bak.exists() {
        // A root-layout prefix renamed away by a failed repair leaves no drive_c for
        // layout detection (wine_prefix then defaults to `pfx`) — probe the
        // root-layout backup location too before giving up.
        let root_bak = sibling_with_suffix(&steam_cfg.root_dir, ".bak");
        if root_bak != bak && root_bak.join("drive_c").is_dir() {
            target = steam_cfg.root_dir.clone();
            bak = root_bak;
        }
    }
    if !bak.exists() {
        return Err(anyhow!(
            "no repair backup to restore: {} does not exist. A `.bak` is created by \
             `aurelia steam-runtime repair` (and deleted by `steam-runtime uninstall`).",
            bak.display()
        ));
    }

    SteamClient::kill_steam_in_prefix(&target);
    #[cfg(unix)]
    SteamClient::kill_wine_processes_in_prefix(&target, true);
    // Give the wineserver a moment to release open file handles before the swap.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    if !target.exists() {
        std::fs::rename(&bak, &target).with_context(|| {
            format!("failed moving {} to {}", bak.display(), target.display())
        })?;
        tracing::info!("Restore: moved {} into place at {}", bak.display(), target.display());
        return Ok(false);
    }

    // Swap current ↔ .bak via a temp name so an interrupted restore never loses
    // either copy.
    let tmp = sibling_with_suffix(&target, ".restore-tmp");
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp)
            .with_context(|| format!("failed removing leftover {}", tmp.display()))?;
    }
    std::fs::rename(&target, &tmp)
        .with_context(|| format!("failed moving {} aside to {}", target.display(), tmp.display()))?;
    if let Err(e) = std::fs::rename(&bak, &target) {
        // Undo so the prefix isn't left missing.
        let _ = std::fs::rename(&tmp, &target);
        return Err(anyhow::Error::new(e).context(format!(
            "failed moving backup {} into place at {}",
            bak.display(),
            target.display()
        )));
    }
    std::fs::rename(&tmp, &bak).with_context(|| {
        format!(
            "restored the backup, but failed moving the old prefix {} to {}",
            tmp.display(),
            bak.display()
        )
    })?;
    tracing::info!("Restore: swapped {} <-> {}", target.display(), bak.display());
    Ok(true)
}

/// Stop the Windows Steam client running in the master prefix — kill its whole Wine
/// session without removing anything. Use to shut down a Steam started by
/// `steam-runtime login` or one left running by a game launch. Returns whether a
/// Steam client was actually running.
pub fn stop_master_steam() -> bool {
    let steam_cfg = crate::core::utils::get_master_steam_config();
    let was_running = SteamClient::is_steam_running_in_prefix(&steam_cfg.wine_prefix);
    SteamClient::kill_steam_in_prefix(&steam_cfg.wine_prefix);
    #[cfg(unix)]
    SteamClient::kill_wine_processes_in_prefix(&steam_cfg.wine_prefix, true);
    was_running
}

/// Remove the master Windows Steam prefix entirely — the opposite of
/// [`install_master_steam`]. Stops any Steam still running in the prefix first (so no
/// files are held open), then deletes the whole master Steam root (the prefix **and**
/// any `.bak` a previous [`repair_master_steam`] left). A no-op if nothing is
/// installed. Unlike `repair`, this keeps no backup — it's the clean-slate path for a
/// corrupted install.
pub async fn uninstall_master_steam() -> Result<()> {
    let steam_cfg = crate::core::utils::get_master_steam_config();

    SteamClient::kill_steam_in_prefix(&steam_cfg.wine_prefix);
    #[cfg(unix)]
    SteamClient::kill_wine_processes_in_prefix(&steam_cfg.wine_prefix, true);
    // Give the wineserver a moment to release open file handles before deletion.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    if steam_cfg.root_dir.exists() {
        std::fs::remove_dir_all(&steam_cfg.root_dir).with_context(|| {
            format!(
                "failed to remove the master Steam prefix at {}",
                steam_cfg.root_dir.display()
            )
        })?;
    }
    Ok(())
}

/// (Re-)start the master Steam client **interactively** so the user can sign in —
/// e.g. after the in-prefix Steam session expired. Unlike a game launch (which starts
/// Steam `-silent`), this brings up the client UI. Any Steam already running in the
/// master prefix is stopped first so a real login window appears instead of the
/// request re-attaching to a silent background instance.
///
/// The in-Wine Steam client keeps its **own** login state in the master prefix,
/// independent of `aurelia login`; this is how you refresh it without reinstalling.
pub async fn relogin_master_steam(config: &LauncherConfig) -> Result<()> {
    let base_dir = config_dir()?;
    let steam_cfg = crate::core::utils::get_master_steam_config();

    let steam_exe = steam_cfg.steam_exe.clone().ok_or_else(|| {
        anyhow!(
            "the Windows Steam runtime is not installed yet (no steam.exe under {}). \
             Run `aurelia steam-runtime install` first.",
            steam_cfg.wine_prefix.display()
        )
    })?;

    let runner_name = config.steam_runtime_runner.to_string_lossy();
    let library_root = PathBuf::from(&config.steam_library_path);
    let wine = crate::core::utils::resolve_steam_runtime_wine(&runner_name, &library_root)?;

    // Stop any running (typically `-silent`) in-prefix Steam so the login UI opens.
    SteamClient::kill_steam_in_prefix(&steam_cfg.wine_prefix);
    #[cfg(unix)]
    SteamClient::kill_wine_processes_in_prefix(&steam_cfg.wine_prefix, true);

    // The CEF login UI needs the runner's dxvk/vkd3d PE libs in the prefix (a
    // bare-wine prefix misses them and the UI crash-loops invisibly).
    crate::core::utils::ensure_steam_runtime_prefix_libs(&wine, &steam_cfg.wine_prefix);

    launch_master_steam(&wine, &steam_exe, &steam_cfg, &base_dir)
}

/// Launch a game that lives **only** in the in-Wine Steam runtime's own library by
/// handing it to the in-Wine Steam client (`steam.exe -applaunch <app_id>`) — exactly
/// how launching it from the in-Wine Steam GUI works.
///
/// Such a game (installed *through* the in-Wine Steam itself — the only route for
/// Family-Shared titles Aurelia cannot download) can't be cold-launched through the
/// Proton pipeline: its Steamworks handshake fails because it expects the full Steam
/// context the running client sets up (registry `ActiveProcess`, a live client to load
/// `steamclient64.dll` against, etc.). Handing it to the running in-Wine Steam gives it
/// exactly that.
///
/// The game then runs as a child of the in-Wine Steam **inside the master prefix**, so
/// Aurelia does not own or track the process directly. We ensure the client is up,
/// dispatch the launch, then (best-effort) block until the game exits so the caller's
/// "Launching…/Finished" flow stays meaningful.
pub async fn launch_game_via_master_steam(
    config: &LauncherConfig,
    app_id: u32,
    install_path: &Path,
) -> Result<()> {
    let base_dir = config_dir()?;
    let steam_cfg = crate::core::utils::get_master_steam_config();
    let steam_exe = steam_cfg.steam_exe.clone().ok_or_else(|| {
        anyhow!(
            "the Windows Steam runtime is not installed (no steam.exe under {}). \
             Run `aurelia steam-runtime install` first.",
            steam_cfg.wine_prefix.display()
        )
    })?;

    let runner_name = config.steam_runtime_runner.to_string_lossy();
    let library_root = PathBuf::from(&config.steam_library_path);
    let wine = crate::core::utils::resolve_steam_runtime_wine(&runner_name, &library_root)?;

    // The in-Wine Steam CEF UI and the game's DXVK both need the runner's PE libs in
    // the prefix (a bare-wine prefix misses them).
    crate::core::utils::ensure_steam_runtime_prefix_libs(&wine, &steam_cfg.wine_prefix);

    // The in-Wine Steam client must be up to authorise the launch and serve DRM. If it
    // isn't, start it silently and wait for it to come up before dispatching.
    if !SteamClient::is_steam_running_in_prefix(&steam_cfg.wine_prefix) {
        tracing::info!("in-Wine Steam not running; starting it before -applaunch");
        launch_master_steam(&wine, &steam_exe, &steam_cfg, &base_dir)?;
        let mut ready = false;
        for _ in 0..40 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if SteamClient::is_steam_running_in_prefix(&steam_cfg.wine_prefix) {
                ready = true;
                break;
            }
        }
        if !ready {
            return Err(anyhow!(
                "the in-Wine Steam client did not come up in time — sign in first with \
                 `aurelia steam-runtime login`, then retry"
            ));
        }
        // A freshly-started client needs a moment more before it can service launches.
        tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    }

    // Dispatch the launch through the in-Wine Steam. With a client already running this
    // forwards the request to it and returns quickly; the game starts as its child.
    let mut cmd = Command::new(&wine);
    cmd.arg(&steam_exe);
    cmd.arg("-applaunch");
    cmd.arg(app_id.to_string());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    apply_master_steam_env(&mut cmd, &steam_cfg, &base_dir)?;
    cmd.spawn()
        .context("failed to dispatch -applaunch to the in-Wine Steam")?;

    // Identify the game by its install-dir basename (the manifest `installdir`), which
    // shows up in the running game's cmdline as `…\common\<installdir>\…exe`.
    let Some(installdir) = install_path.file_name().map(|n| n.to_string_lossy().to_string())
    else {
        return Ok(());
    };

    // Wait (bounded) for the game to appear, then block until it exits. If it never
    // appears (client showed an error, game is updating, or it exited instantly), don't
    // hang the caller.
    let mut appeared = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if SteamClient::is_game_running_in_prefix(&steam_cfg.wine_prefix, &installdir) {
            appeared = true;
            break;
        }
    }
    if !appeared {
        tracing::warn!(
            "game process for app {app_id} did not appear within 30s of -applaunch; \
             the in-Wine Steam may still be starting it, updating, or showing a prompt"
        );
        return Ok(());
    }
    while SteamClient::is_game_running_in_prefix(&steam_cfg.wine_prefix, &installdir) {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    Ok(())
}

/// True when `path` looks like a real Windows executable.
///
/// PE binaries open with the `MZ` DOS header. The previous code only checked
/// `exists()`, so a CDN error page or a download interrupted midway was cached as
/// `SteamSetup.exe` and reused forever — every later install would "succeed" at the
/// download step and then hand wine a file it could not execute.
pub fn is_valid_setup_exe(path: &Path) -> bool {
    use std::io::Read;
    let mut header = [0u8; 2];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .is_ok()
        && &header == b"MZ"
}

/// Download `SteamSetup.exe` unless a valid one is already cached.
async fn ensure_steam_setup(path: &Path) -> Result<()> {
    if is_valid_setup_exe(path) {
        tracing::info!("Using cached SteamSetup.exe at {}", path.display());
        return Ok(());
    }
    if path.exists() {
        tracing::warn!(
            "Cached {} is not a valid Windows executable — re-downloading",
            path.display()
        );
    }
    download_steam_setup(path).await
}

async fn download_steam_setup(path: &Path) -> Result<()> {
    tracing::info!("Downloading SteamSetup.exe...");
    let url = "https://cdn.akamai.steamstatic.com/client/installer/SteamSetup.exe";
    let bytes = reqwest::get(url)
        .await
        .context("Failed to reach the Steam CDN to download SteamSetup.exe")?
        .error_for_status()
        .context("Steam CDN rejected the SteamSetup.exe download")?
        .bytes()
        .await
        .context("Failed to read the SteamSetup.exe response body")?;

    if bytes.len() < 2 || &bytes[..2] != b"MZ" {
        return Err(anyhow!(
            "Downloaded SteamSetup.exe is not a Windows executable ({} bytes from {url})",
            bytes.len()
        ));
    }

    // Write to a temp file and rename, so an interrupted write can never leave a
    // truncated SteamSetup.exe behind.
    let tmp = path.with_extension("exe.part");
    std::fs::write(&tmp, &bytes)
        .with_context(|| format!("failed writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("failed moving {} into place at {}", tmp.display(), path.display()))?;
    Ok(())
}

