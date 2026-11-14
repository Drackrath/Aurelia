//! Tracks games launched by Aurelia so a separate `aurelia stop <app_id>`
//! invocation can find and terminate them.
//!
//! `aurelia play` blocks in the foreground process while the game runs, so the
//! PID of the launched process can't be queried from another invocation. To make
//! `stop` work across processes, each launch records a small JSON file under
//! `~/.config/Aurelia/running/<app_id>.json` holding the child PID (and, for a
//! per-game Proton/Wine launch, the WINEPREFIX the game runs in). The launching
//! process removes the file when the game exits.
//!
//! # Recovery contract
//!
//! A record can outlive its launch: `aurelia play` may be killed or crash, the
//! machine may reboot, and PIDs are recycled. A record is therefore never trusted
//! on its PID alone:
//!
//! - Each record pairs the PID with that process's start time. The recorded PID
//!   is only treated as the game (listed as running, or signalled by `stop`)
//!   while a process with *both* the same PID and the same start time exists.
//! - A record whose PID no longer matches is reconciled against the processes
//!   carrying the app id in their environment (see [`processes_for_app`]). If
//!   one survives, the record is re-pointed at it; otherwise the record is stale
//!   and is removed without signalling anything.
//! - Records written before start times were tracked carry none and are
//!   reconciled the same way, never acted on by PID.
//! - Records are replaced atomically (write to a temp file, then rename), so a
//!   crash mid-write leaves the previous record rather than a truncated one.

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
    /// Start time of `pid` as the OS reports it (seconds since the Unix epoch).
    /// Together with `pid` this identifies the process, since PIDs are recycled.
    /// `None` when it could not be read, or on records written before it existed;
    /// such a record never authorises signalling `pid`.
    #[serde(default)]
    pub pid_start_time: Option<u64>,
    /// The per-game WINEPREFIX the game runs in, when launched through
    /// Proton/Wine. Present only for per-game (compatdata) prefixes — never the
    /// shared master prefix, which also hosts the Steam client. Lets `stop`
    /// sweep leftover wine processes inside the prefix (Linux).
    #[serde(default)]
    pub wineprefix: Option<PathBuf>,
    /// Launched with the stand-in steamclient.so.
    #[serde(default)]
    pub stand_in: bool,
    /// Sandbox mount namespace inode.
    #[serde(default)]
    pub mnt_ns: Option<u64>,
}

impl RunningGame {
    /// A record for a just-spawned process, capturing its start time.
    pub fn new(app_id: u32, name: String, pid: u32, wineprefix: Option<PathBuf>) -> Self {
        Self {
            app_id,
            name,
            pid,
            pid_start_time: process_start_time(pid),
            wineprefix,
            stand_in: false,
            mnt_ns: None,
        }
    }

    /// Marks a stand-in launch.
    pub fn with_stand_in(mut self, stand_in: bool) -> Self {
        self.stand_in = stand_in;
        self
    }

    /// Re-point the record at `pid` (a surviving game process), refreshing the
    /// start time to match.
    pub fn set_pid(&mut self, pid: u32) {
        self.pid = pid;
        self.pid_start_time = process_start_time(pid);
    }

    /// Whether the recorded PID still names the process this record was written
    /// for: same PID *and* same start time. False for a record without a start
    /// time, so a recycled PID is never mistaken for the game.
    pub fn owns_pid(&self) -> bool {
        self.pid_start_time
            .is_some_and(|t| process_start_time(self.pid) == Some(t))
    }
}

/// Parent of `pid` from `/proc`.
pub fn parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Fields after the `(comm)` name.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// `pid` is `ancestor` or below it.
pub fn descends_from(pid: u32, ancestor: u32, parent: impl Fn(u32) -> Option<u32>) -> bool {
    let mut current = pid;
    // Bounded walk; init ends it.
    for _ in 0..256 {
        if current == ancestor {
            return true;
        }
        match parent(current) {
            Some(p) if p != current && p != 0 => current = p,
            _ => return false,
        }
    }
    false
}

/// Mount namespace inode of `pid`.
pub fn mnt_ns(pid: u32) -> Option<u64> {
    let link = std::fs::read_link(format!("/proc/{pid}/ns/mnt")).ok()?;
    parse_ns_link(link.to_str()?)
}

/// Inode from `mnt:[4026532792]`.
pub fn parse_ns_link(link: &str) -> Option<u64> {
    link.strip_prefix("mnt:[")?.strip_suffix(']')?.parse().ok()
}

/// This process's (host) namespace.
pub fn host_mnt_ns() -> Option<u64> {
    mnt_ns(std::process::id())
}

/// Direct children of `pid` from `/proc`.
pub fn children_of(pid: u32) -> Vec<u32> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&child| parent_pid(child) == Some(pid))
        .collect()
}

/// Namespace `pid` created, if any.
///
/// Shared with its parent: inherited.
pub fn own_namespace(
    pid: u32,
    parent: &impl Fn(u32) -> Option<u32>,
    ns_of: &impl Fn(u32) -> Option<u64>,
) -> Option<u64> {
    let ns = ns_of(pid)?;
    let inherited = parent(pid).and_then(ns_of) == Some(ns);
    (!inherited).then_some(ns)
}

/// Namespaces a launch's processes use.
///
/// Never the launcher's or host's.
pub fn launch_namespaces(
    game: &RunningGame,
    parent: &impl Fn(u32) -> Option<u32>,
    ns_of: &impl Fn(u32) -> Option<u64>,
    children: &impl Fn(u32) -> Vec<u32>,
    host_ns: Option<u64>,
) -> Vec<u64> {
    let mut out: Vec<u64> = game.mnt_ns.into_iter().collect();
    for pid in std::iter::once(game.pid).chain(children(game.pid)) {
        if let Some(ns) = own_namespace(pid, parent, ns_of)
            && !out.contains(&ns)
        {
            out.push(ns);
        }
    }
    out.retain(|ns| Some(*ns) != host_ns);
    out
}

/// Launch owning `peer`: ancestry, else namespace.
///
/// The host namespace never matches.
pub fn match_launch(
    peer: u32,
    launches: &[RunningGame],
    parent: impl Fn(u32) -> Option<u32>,
    ns_of: impl Fn(u32) -> Option<u64>,
    children: impl Fn(u32) -> Vec<u32>,
    host_ns: Option<u64>,
) -> Option<u32> {
    let stand_in = || launches.iter().filter(|g| g.stand_in);
    if let Some(g) = stand_in().find(|g| descends_from(peer, g.pid, &parent)) {
        return Some(g.app_id);
    }
    // Wine double-forks out of the tree.
    let peer_ns = ns_of(peer).filter(|ns| Some(*ns) != host_ns)?;
    stand_in()
        .find(|g| launch_namespaces(g, &parent, &ns_of, &children, host_ns).contains(&peer_ns))
        .map(|g| g.app_id)
}

/// Stand-in launch app owning `pid`.
pub fn stand_in_launch_of(pid: u32) -> Option<u32> {
    match_launch(
        pid,
        &list_active(),
        parent_pid,
        mnt_ns,
        children_of,
        host_mnt_ns(),
    )
}

/// Records the sandbox namespace once visible.
///
/// Polls briefly off-thread after spawn.
pub fn capture_launch_namespace(app_id: u32, pid: u32) {
    let host = host_mnt_ns();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            // Only a namespace bwrap created.
            let sandbox = children_of(pid)
                .into_iter()
                .filter_map(|c| own_namespace(c, &parent_pid, &mnt_ns))
                .find(|ns| Some(*ns) != host);
            if let Some(ns) = sandbox {
                if let Some(mut game) = load(app_id).filter(|g| g.pid == pid) {
                    game.mnt_ns = Some(ns);
                    let _ = record_launch(&game);
                }
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
}

/// Start time of `pid` (seconds since the Unix epoch), or `None` if no such
/// process exists.
pub fn process_start_time(pid: u32) -> Option<u64> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.process(pid).map(|p| p.start_time())
}

fn running_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("running"))
}

fn record_path(app_id: u32) -> Result<PathBuf> {
    Ok(running_dir()?.join(format!("{app_id}.json")))
}

/// Persist that `game` is now running so `stop` can find it later.
/// Re-points the record, keeping its namespace.
pub fn update_launch(game: &RunningGame) -> Result<()> {
    let mut game = game.clone();
    if game.mnt_ns.is_none() && game.stand_in {
        game.mnt_ns = load(game.app_id)
            .filter(|old| old.stand_in)
            .and_then(|old| old.mnt_ns);
    }
    record_launch(&game)
}

pub fn record_launch(game: &RunningGame) -> Result<()> {
    let dir = running_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("failed creating {}", dir.display()))?;
    let path = record_path(game.app_id)?;
    let body = serde_json::to_string_pretty(game)?;
    crate::core::atomic::write_atomic(&path, body.as_bytes())
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

/// Every game Aurelia currently has a record for, live or stale. Use
/// [`list_active`] for the ones still running.
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

/// Every live process belonging to `app_id`, identified by
/// `STEAM_COMPAT_APP_ID=<id>` (Proton) or `SteamAppId=<id>` (native launches)
/// in its environment. The recorded PID alone is not authoritative: Proton
/// re-parents the game away from the spawned runner, and script launchers
/// (`hl.sh`-style) exit while the game keeps running.
pub fn processes_for_app(app_id: u32) -> Vec<u32> {
    #[cfg(unix)]
    {
        let compat = format!("STEAM_COMPAT_APP_ID={app_id}");
        let native = format!("SteamAppId={app_id}");
        let mut pids = Vec::new();
        let Ok(proc_dir) = std::fs::read_dir("/proc") else {
            return pids;
        };
        for entry in proc_dir.flatten() {
            let path = entry.path();
            let Some(pid) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(environ) = std::fs::read(path.join("environ")) else {
                continue;
            };
            // NUL-separated KEY=VALUE entries; exact match so 70 never matches 700.
            if environ
                .split(|&b| b == 0)
                .any(|e| e == compat.as_bytes() || e == native.as_bytes())
            {
                pids.push(pid);
            }
        }
        pids
    }
    #[cfg(not(unix))]
    {
        let _ = app_id;
        Vec::new()
    }
}

/// The games Aurelia is running whose process is still alive. Records whose
/// process has already exited are pruned as a side effect, so stale launches
/// (an interrupted `aurelia play`, a reboot) don't linger in the listing.
///
/// A record that no longer owns its PID does not mean the game is gone — the
/// record holds the directly spawned process, which wrappers and script launchers
/// outlive — so it is only pruned once no process carries the app id either.
/// When the game survives its wrapper, the record is re-pointed at a live
/// process so `stop` still has a target.
pub fn list_active() -> Vec<RunningGame> {
    list().into_iter().filter_map(reconcile).collect()
}

/// Apply the recovery contract to one record: keep it while it owns its PID,
/// re-point it at a surviving app process, or remove it as stale (`None`).
pub fn reconcile(mut game: RunningGame) -> Option<RunningGame> {
    if game.owns_pid() {
        return Some(game);
    }
    match processes_for_app(game.app_id).first() {
        Some(&pid) => {
            game.set_pid(pid);
            let _ = record_launch(&game);
            Some(game)
        }
        None => {
            clear(game.app_id);
            None
        }
    }
}

#[cfg(test)]
#[path = "running_tests.rs"]
mod tests;
