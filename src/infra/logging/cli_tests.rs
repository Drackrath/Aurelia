use super::*;

#[test]
fn directives_escalate_with_verbosity() {
    assert_eq!(default_directives(0), "warn");
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

/// Run `f` inside a relay scope of `verbosity` under a subscriber made only of the
/// daemon's relay layers; return what was relayed to the client's stderr.
async fn relayed_at(verbosity: u8, f: impl FnOnce()) -> String {
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::SubscriberExt;

    let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink_seen = Arc::clone(&seen);
    let relay = super::super::relay::Relay::new(
        verbosity,
        Arc::new(move |_, bytes: &[u8]| sink_seen.lock().unwrap().extend_from_slice(bytes)),
    );
    let subscriber = tracing_subscriber::registry().with(relay_layers());
    let _guard = tracing::subscriber::set_default(subscriber);
    super::super::relay::scoped(relay, async { f() }).await;
    let out = String::from_utf8_lossy(&seen.lock().unwrap()).into_owned();
    out
}

// The only test touching the global watcher registry, so it can't race another.
#[tokio::test]
async fn daemon_watchers_get_daemon_events_only_while_watching() {
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::SubscriberExt;

    let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink_seen = Arc::clone(&seen);
    let watcher = super::super::relay::Relay::new(
        1,
        Arc::new(move |_, bytes: &[u8]| sink_seen.lock().unwrap().extend_from_slice(bytes)),
    );
    let request = watcher.clone();
    let subscriber = tracing_subscriber::registry()
        .with(relay_layers())
        .with(daemon_watch_layers());
    let _guard = tracing::subscriber::set_default(subscriber);

    let watch = super::super::relay::watch_daemon(watcher);
    tracing::debug!("daemon-debug");
    tracing::trace!("daemon-trace");
    // A request's own events reach it through its relay, not a second time as daemon output.
    super::super::relay::scoped(request, async { tracing::info!("request-info") }).await;
    drop(watch);
    tracing::info!("after-unwatch");

    let out = String::from_utf8_lossy(&seen.lock().unwrap()).into_owned();
    assert!(
        out.contains("[daemon] ") && out.contains("daemon-debug"),
        "{out}"
    );
    assert!(!out.contains("daemon-trace"), "{out}");
    assert_eq!(out.matches("request-info").count(), 1, "{out}");
    assert!(!out.contains("after-unwatch"), "{out}");
}

#[tokio::test]
async fn relay_follows_the_requesters_verbosity() {
    let quiet = relayed_at(0, || {
        tracing::warn!("warn-line");
        tracing::info!("info-line");
    })
    .await;
    assert!(quiet.contains("warn-line"));
    assert!(!quiet.contains("info-line"));

    let verbose = relayed_at(1, || {
        tracing::info!("info-line");
        tracing::debug!("debug-line");
    })
    .await;
    assert!(verbose.contains("info-line") && verbose.contains("debug-line"));

    // Exactly one layer handles each level, so an event isn't relayed twice.
    let trace = relayed_at(7, || tracing::trace!("trace-line")).await;
    assert_eq!(trace.matches("trace-line").count(), 1);
}
