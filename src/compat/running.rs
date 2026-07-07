//! Tracks games launched by Aurelia so a separate `aurelia stop <app_id>`
//! invocation can find and terminate them.
//!
//! `aurelia play` blocks in the foreground process while the game runs, so the
//! PID of the launched process can't be queried from another invocation. To make
//! `stop` work across processes, each launch records a small JSON file under
//! `~/.config/Aurelia/running/<app_id>.json` holding the child PID (and, for a
//! per-game Proton/Wine launch, the WINEPREFIX the game runs in). The launching
//! process removes the file when the game exits.

use crate::core::config::config_dir;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningGame {
    pub app_id: u32,
    pub name: String,
    /// PID of the process Aurelia spawned for this game. On Windows this is the
    /// game executable; for a Proton/Wine launch it is the runner process.
    pub pid: u32,
    /// The per-game WINEPREFIX the game runs in, when launched through
    /// Proton/Wine. Present only for per-game (compatdata) prefixes — never the
    /// shared master prefix, which also hosts the Steam client. Lets `stop`
    /// sweep leftover wine processes inside the prefix (Linux).
    #[serde(default)]
    pub wineprefix: Option<PathBuf>,
}

fn running_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("running"))
}

fn record_path(app_id: u32) -> Result<PathBuf> {
    Ok(running_dir()?.join(format!("{app_id}.json")))
}

/// Persist that `game` is now running so `stop` can find it later.
pub fn record_launch(game: &RunningGame) -> Result<()> {
    let dir = running_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed creating {}", dir.display()))?;
    let path = record_path(game.app_id)?;
    let body = serde_json::to_string_pretty(game)?;
    std::fs::write(&path, body).with_context(|| format!("failed writing {}", path.display()))?;
    Ok(())
}

/// Load the running-game record for `app_id`, if one exists.
pub fn load(app_id: u32) -> Option<RunningGame> {
    let path = record_path(app_id).ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Remove the running-game record for `app_id` (no-op if absent).
pub fn clear(app_id: u32) {
    if let Ok(path) = record_path(app_id) {
        let _ = std::fs::remove_file(path);
    }
}

/// Every game Aurelia currently believes is running.
pub fn list() -> Vec<RunningGame> {
    let Ok(dir) = running_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut games: Vec<RunningGame> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|raw| serde_json::from_str(&raw).ok())
        .collect();
    games.sort_by(|a, b| a.name.cmp(&b.name));
    games
}

/// Whether the process behind a recorded launch is still alive. A launch that was
/// interrupted (e.g. `aurelia play` killed with Ctrl-C) can leave its record
/// behind after the process is gone, so liveness is checked against the OS.
pub fn is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // `/proc/<pid>` exists for every live process and is cheaper (and needs no
        // libc/unsafe) than a signal-0 probe.
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        // No cheap cross-process liveness check here; treat the record as live.
        true
    }
}

/// The games Aurelia is running whose process is still alive. Records whose
/// process has already exited are pruned as a side effect, so stale launches
/// (e.g. an interrupted `aurelia play`) don't linger in the listing.
pub fn list_active() -> Vec<RunningGame> {
    let mut alive = Vec::new();
    for game in list() {
        if is_alive(game.pid) {
            alive.push(game);
        } else {
            clear(game.app_id);
        }
    }
    alive
}
