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
//! * Default: warnings and errors only; a launched game's own output is hidden.
//! * `-v`: Aurelia's progress and debug logs, plus the game/Proton output.
//! * `-vv` / `-vvv`: progressively unmute `steam-vent` and friends.
//! * `RUST_LOG` or `AURELIA_LOG`: full manual control, overriding the flags.
//!
//! The daemon uses [`init_daemon_logging`] instead, which also relays each forwarded
//! command's events to its client at that client's `-v` level.

use std::io::IsTerminal;
use tracing_subscriber::EnvFilter;

/// Initialise global tracing for the CLI, writing to **stderr** so stdout stays
/// clean for `--json` output.
///
/// `verbosity` is the count of `-v` flags passed on the command line (0 = none).
/// An explicit `AURELIA_LOG`/`RUST_LOG` environment variable takes precedence
/// over `verbosity`.
pub fn init_cli_logging(verbosity: u8) {
    super::relay::set_local_verbosity(verbosity);
    let filter = resolve_filter(verbosity);

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        // Only colourise when stderr is an interactive terminal.
        .with_ansi(std::io::stderr().is_terminal())
        .with_target(verbosity >= 2)
        // Its `eprintln!` panics on a closed stderr.
        .log_internal_errors(false)
        .init();
}

/// Initialise global tracing for the session daemon (`aurelia daemon`).
///
/// The daemon's own events go to its stderr as in [`init_cli_logging`]. On top, one
/// relay layer per verbosity level formats events raised while serving a forwarded
/// command into that request's stderr, filtered by the *requester's* `-v` count —
/// so `aurelia play -vv` shows the same logs whether or not a daemon ran it.
pub fn init_daemon_logging(verbosity: u8) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::Layer;

    let own = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_target(verbosity >= 2)
        .log_internal_errors(false)
        .with_filter(resolve_filter(verbosity));

    tracing_subscriber::registry()
        .with(own)
        .with(relay_layers())
        .with(daemon_watch_layers())
        .init();
}

/// The highest `-v` count with its own directive set; more `-v`s behave like it.
const MAX_VERBOSITY: u8 = 3;

/// One relay fmt layer per verbosity level (see [`init_daemon_logging`]). Layers
/// differ in filter and in whether targets are shown, mirroring the local CLI.
fn relay_layers<S>() -> Vec<Box<dyn tracing_subscriber::Layer<S> + Send + Sync>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    use tracing_subscriber::Layer;

    (0..=MAX_VERBOSITY)
        .map(|level| {
            tracing_subscriber::fmt::layer()
                .with_writer(super::relay::RelayWriter)
                // The client's terminal is unknown here; keep relayed logs plain.
                .with_ansi(false)
                .with_target(level >= 2)
                .log_internal_errors(false)
                .with_filter(RelayFilter {
                    level,
                    directives: EnvFilter::new(default_directives(level)),
                    scope: RelayScope::Request,
                })
                .boxed()
        })
        .collect()
}

/// One layer per verbosity level relaying the daemon's *own* events (those raised
/// outside any request) to requests that [`watch_daemon`](super::relay::watch_daemon).
fn daemon_watch_layers<S>() -> Vec<Box<dyn tracing_subscriber::Layer<S> + Send + Sync>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    use tracing_subscriber::Layer;

    (0..=MAX_VERBOSITY)
        .map(|level| {
            tracing_subscriber::fmt::layer()
                .with_writer(super::relay::DaemonWatchWriter { level, max: MAX_VERBOSITY })
                .with_ansi(false)
                .with_target(level >= 2)
                .log_internal_errors(false)
                .with_filter(RelayFilter {
                    level,
                    directives: EnvFilter::new(default_directives(level)),
                    scope: RelayScope::Daemon,
                })
                .boxed()
        })
        .collect()
}

/// Which events a [`RelayFilter`] considers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RelayScope {
    /// Events raised while serving a request whose requester asked for the level.
    Request,
    /// Events raised outside any request, while some watcher asked for the level.
    Daemon,
}

/// Passes an event in `scope` for `level`, and only if `level`'s directives enable it.
struct RelayFilter {
    level: u8,
    directives: EnvFilter,
    scope: RelayScope,
}

impl<S> tracing_subscriber::layer::Filter<S> for RelayFilter {
    fn enabled(
        &self,
        meta: &tracing::Metadata<'_>,
        cx: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        let in_scope = match (self.scope, super::relay::current_verbosity()) {
            (RelayScope::Request, Some(v)) => v.min(MAX_VERBOSITY) == self.level,
            (RelayScope::Daemon, None) => {
                super::relay::has_daemon_watcher(self.level, MAX_VERBOSITY)
            }
            _ => false,
        };
        in_scope
            && <EnvFilter as tracing_subscriber::layer::Filter<S>>::enabled(&self.directives, meta, cx)
    }

    // Whether an event is relayed depends on the task it fires in, so never let a
    // callsite be cached as "never" (the default `sometimes` interest), and don't
    // cap the global max level by this layer.
    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        None
    }
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
/// At the default level only warnings and errors are logged — progress chatter
/// (session restore, library fetch, cloud sync) stays behind `-v`; each further
/// `-v` unmutes more of the stack so a hang inside a Steam connection or RPC
/// becomes visible.
fn default_directives(verbosity: u8) -> &'static str {
    match verbosity {
        0 => "warn",
        1 => "debug,steam_vent=info,steam_vent_proto=info",
        2 => "debug,steam_vent=debug,steam_vent_proto=debug",
        _ => "trace",
    }
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
