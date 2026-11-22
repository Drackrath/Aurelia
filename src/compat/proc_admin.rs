//! Process discovery and termination.

use sysinfo::{Pid, System};

/// A running `aurelia` process other than the current one.
pub struct AureliaProcess {
    pub pid: u32,
    /// Whether this is a session daemon (`aurelia daemon`) rather than a thin client
    /// or one-off command.
    pub is_daemon: bool,
    /// The full command line, for display.
    pub command: String,
}

/// True if a process executable name is `aurelia` (with or without a `.exe`
/// suffix), case-insensitively.
fn is_aurelia_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower) == "aurelia"
}

/// Enumerate running `aurelia` processes, excluding the current process.
pub fn find_aurelia_processes(is_daemon: impl Fn(&[String]) -> bool) -> Vec<AureliaProcess> {
    let me = std::process::id();
    let Ok(sys) = System::new_all() else {
        tracing::warn!("could not read the process list");
        return Vec::new();
    };

    let mut out: Vec<AureliaProcess> = sys
        .processes()
        .iter()
        .filter(|(pid, _)| pid.as_u32() != me)
        .filter(|(_, p)| is_aurelia_name(&p.name().to_string_lossy()))
        .map(|(pid, p)| {
            let cmd: Vec<String> = p
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect();
            AureliaProcess {
                pid: pid.as_u32(),
                is_daemon: is_daemon(&cmd),
                command: cmd.join(" "),
            }
        })
        .collect();
    out.sort_by_key(|p| p.pid);
    out
}

/// Terminate the given pids, returning how many were successfully signalled.
pub fn kill_pids(pids: &[u32]) -> usize {
    if pids.is_empty() {
        return 0;
    }
    let Ok(sys) = System::new_all() else {
        tracing::warn!("could not read the process list");
        return 0;
    };
    pids.iter()
        .filter_map(|&pid| sys.process(Pid::from_u32(pid)))
        .filter(|p| p.kill())
        .count()
}

/// Terminate the process tree rooted at `pid`. With `force`, kill immediately;
/// otherwise ask it to exit first (SIGTERM) and only SIGKILL the stragglers after
/// a short grace period so the game can shut down cleanly.
pub fn kill_process_tree(pid: u32, force: bool) {
    #[cfg(target_os = "windows")]
    {
        let mut args = vec!["/PID".to_string(), pid.to_string(), "/T".to_string()];
        if force {
            args.push("/F".to_string());
        }
        let _ = std::process::Command::new("taskkill").args(&args).output();
    }

    #[cfg(unix)]
    {
        let tree = process_tree_pids(pid as i32);
        if force {
            for &p in &tree {
                unsafe {
                    libc::kill(p, libc::SIGKILL);
                }
            }
        } else {
            for &p in &tree {
                unsafe {
                    libc::kill(p, libc::SIGTERM);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
            for &p in &tree {
                unsafe {
                    libc::kill(p, libc::SIGKILL);
                }
            }
        }
    }
}

/// Root pid plus live descendants.
#[cfg(unix)]
fn process_tree_pids(root: i32) -> Vec<i32> {
    let mut parent_of: Vec<(i32, i32)> = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/proc") {
        for entry in dir.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|s| s.parse::<i32>().ok()) else {
                continue;
            };
            let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
                continue;
            };
            if let Some(ppid) = status
                .lines()
                .find_map(|l| l.strip_prefix("PPid:"))
                .and_then(|v| v.trim().parse::<i32>().ok())
            {
                parent_of.push((pid, ppid));
            }
        }
    }
    let mut tree = vec![root];
    let mut i = 0;
    while i < tree.len() {
        let cur = tree[i];
        for &(pid, ppid) in &parent_of {
            if ppid == cur && !tree.contains(&pid) {
                tree.push(pid);
            }
        }
        i += 1;
    }
    tree
}

#[cfg(test)]
mod tests;
