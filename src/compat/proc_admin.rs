//! Discovery and termination of running `aurelia` processes, backing the
//! `aurelia kill` and `aurelia daemon stop|list` commands.

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

/// Clap-parsed argv is bare `daemon`.
fn cmd_is_daemon(cmd: &[String]) -> bool {
    use clap::Parser;
    crate::cli::Cli::try_parse_from(cmd)
        .is_ok_and(|c| matches!(c.command, crate::cli::Command::Daemon { command: None, .. }))
}

/// Enumerate running `aurelia` processes, excluding the current process.
pub fn find_aurelia_processes() -> Vec<AureliaProcess> {
    let me = std::process::id();
    let sys = System::new_all();

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
                is_daemon: cmd_is_daemon(&cmd),
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
    let sys = System::new_all();
    pids.iter()
        .filter_map(|&pid| sys.process(Pid::from_u32(pid)))
        .filter(|p| p.kill())
        .count()
}

#[cfg(test)]
#[path = "proc_admin_tests.rs"]
mod tests;
