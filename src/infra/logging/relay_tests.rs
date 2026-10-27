use super::*;
use std::sync::Mutex;

/// A relay recording everything sent to it.
fn recording_relay(verbosity: u8) -> (Relay, Arc<Mutex<Vec<(RelayStream, Vec<u8>)>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink_seen = Arc::clone(&seen);
    let relay = Relay::new(
        verbosity,
        Arc::new(move |stream, bytes: &[u8]| sink_seen.lock().unwrap().push((stream, bytes.to_vec()))),
    );
    (relay, seen)
}

fn echo_command() -> std::process::Command {
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "echo out& echo err 1>&2"]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "echo out; echo err >&2"]);
        cmd
    }
}

fn collected(seen: &Mutex<Vec<(RelayStream, Vec<u8>)>>, stream: RelayStream) -> String {
    let bytes: Vec<u8> = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|(s, _)| *s == stream)
        .flat_map(|(_, b)| b.clone())
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

#[tokio::test]
async fn child_output_is_relayed_inside_a_scope() {
    let (relay, seen) = recording_relay(1);
    let mut child = scoped(relay, async {
        let mut cmd = echo_command();
        cmd.stdout(child_stdio()).stderr(child_stdio());
        cmd.spawn().map(attach).unwrap()
    })
    .await;
    child.wait().unwrap();
    // The pumps finish at EOF, shortly after the child exits.
    for _ in 0..50 {
        if !collected(&seen, RelayStream::Stdout).is_empty()
            && !collected(&seen, RelayStream::Stderr).is_empty()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(collected(&seen, RelayStream::Stdout), "out");
    assert_eq!(collected(&seen, RelayStream::Stderr), "err");
}

#[tokio::test]
async fn child_output_is_dropped_without_verbose() {
    let (relay, seen) = recording_relay(0);
    let mut child = scoped(relay, async {
        let mut cmd = echo_command();
        cmd.stdout(child_stdio()).stderr(child_stdio());
        cmd.spawn().map(attach).unwrap()
    })
    .await;
    child.wait().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn outside_a_scope_nothing_is_relayed() {
    assert!(current().is_none());
    assert!(!send_current(RelayStream::Stderr, b"x"));
}
