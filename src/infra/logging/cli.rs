//! Global logging setup for the `aurelia` command-line binary.
//!
//! Previously the binary initialised tracing with a bare
//! `tracing_subscriber::fmt()...init()`. That fixed the level at `info` for
//! every crate, which meant the Steam networking stack (`steam-vent`, which logs
//! its connection/RPC activity at `debug`) produced **no** output. As a result a
//! command that stalled inside a Steam RPC — e.g. `list` waiting on
//! `Player.GetOwnedGames` — appeared to hang with nothing on screen.
//!
//! This module centralises the configuration and makes the verbosity tunable so
//! a stuck command can be diagnosed:
//!
//! * Default: Aurelia's own progress at `info`; noisy network crates quieted.
//! * `-v` / `-vv` / `-vvv`: progressively unmute `steam-vent` and friends.
//! * `RUST_LOG` or `AURELIA_LOG`: full manual control, overriding the flags.

use std::io::IsTerminal;
use tracing_subscriber::EnvFilter;

/// Initialise global tracing for the CLI, writing to **stderr** so stdout stays
/// clean for `--json` output.
///
/// `verbosity` is the count of `-v` flags passed on the command line (0 = none).
/// An explicit `AURELIA_LOG`/`RUST_LOG` environment variable takes precedence
/// over `verbosity`.
pub fn init_cli_logging(verbosity: u8) {
    let filter = resolve_filter(verbosity);

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        // Only colourise when stderr is an interactive terminal.
        .with_ansi(std::io::stderr().is_terminal())
        .with_target(verbosity >= 2)
        .init();
}

/// Build the [`EnvFilter`]. An explicit environment variable wins; otherwise the
/// directives are derived from the `-v` count.
fn resolve_filter(verbosity: u8) -> EnvFilter {
    // `AURELIA_LOG` is checked first as an Aurelia-specific override, then the
    // conventional `RUST_LOG`.
    EnvFilter::try_from_env("AURELIA_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new(default_directives(verbosity)))
}

/// Map a `-v` count to a set of env-filter directives.
///
/// At the default level Aurelia's own logs are shown but the chatty networking
/// crates are held at `warn`; each `-v` unmutes more of the stack so a hang
/// inside a Steam connection or RPC becomes visible.
fn default_directives(verbosity: u8) -> &'static str {
    match verbosity {
        0 => "info,steam_vent=warn,steam_vent_proto=warn",
        1 => "debug,steam_vent=info,steam_vent_proto=info",
        2 => "debug,steam_vent=debug,steam_vent_proto=debug",
        _ => "trace",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directives_escalate_with_verbosity() {
        assert!(default_directives(0).contains("steam_vent=warn"));
        assert!(default_directives(1).contains("steam_vent=info"));
        assert!(default_directives(2).contains("steam_vent=debug"));
        assert_eq!(default_directives(3), "trace");
        assert_eq!(default_directives(9), "trace");
    }

    #[test]
    fn directives_are_valid_env_filters() {
        // Each directive string must parse as an EnvFilter or the CLI would
        // panic at startup.
        for v in 0..=3 {
            EnvFilter::new(default_directives(v));
        }
    }
}
