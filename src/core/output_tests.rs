use super::*;

#[tokio::test]
async fn one_chunk_yields_each_line_then_eof() {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut stdin = DaemonStdin::new(rx);
    tx.send(b"a\nb\n".to_vec()).unwrap();
    drop(tx);
    assert_eq!(stdin.read_line_opt().await.unwrap().as_deref(), Some("a"));
    assert_eq!(stdin.read_line_opt().await.unwrap().as_deref(), Some("b"));
    assert_eq!(stdin.read_line_opt().await.unwrap(), None);
}

#[tokio::test]
async fn cancelled_read_loses_nothing() {
    let (tx, rx) = mpsc::unbounded_channel();
    let shared = Arc::new(Mutex::new(DaemonStdin::new(rx)));

    // Cancel a pending read via select.
    let reader = shared.clone();
    tokio::select! {
        _ = async move { reader.lock().await.read_line_opt().await } => {
            panic!("no data was sent yet");
        }
        _ = tokio::task::yield_now() => {}
    }

    tx.send(b"cancel\n".to_vec()).unwrap();
    let line = shared.lock().await.read_line_opt().await.unwrap();
    assert_eq!(line.as_deref(), Some("cancel"));
}

#[tokio::test]
async fn trailing_line_without_newline_returned_at_eof() {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut stdin = DaemonStdin::new(rx);
    tx.send(b"x\nlast".to_vec()).unwrap();
    drop(tx);
    assert_eq!(stdin.read_line_opt().await.unwrap().as_deref(), Some("x"));
    assert_eq!(
        stdin.read_line_opt().await.unwrap().as_deref(),
        Some("last")
    );
    assert_eq!(stdin.read_line_opt().await.unwrap(), None);
}

#[tokio::test]
async fn pump_reader_feeds_shared_buffer() {
    let (tx, rx) = mpsc::unbounded_channel();
    let input: &[u8] = b"x\ncancel\n";
    std::thread::spawn(move || pump_reader(input, tx))
        .join()
        .unwrap();
    let mut stdin = DaemonStdin::new(rx);
    assert_eq!(stdin.read_line_opt().await.unwrap().as_deref(), Some("x"));
    assert_eq!(
        stdin.read_line_opt().await.unwrap().as_deref(),
        Some("cancel")
    );
    assert_eq!(stdin.read_line_opt().await.unwrap(), None);
}

#[tokio::test]
async fn user_path_uses_scoped_client_cwd() {
    let abs = Path::new("/etc/hosts");
    assert_eq!(user_path(abs), abs);

    // Outside a scope: process cwd.
    let here = std::env::current_dir().unwrap();
    assert_eq!(user_path(Path::new("a.jpg")), here.join("a.jpg"));

    let (out_tx, _out_rx) = mpsc::unbounded_channel();
    let (_in_tx, in_rx) = mpsc::unbounded_channel();
    let ctx = OutputCtx::new(out_tx, in_rx, Some(PathBuf::from("/home/u/shots")));
    let (rel, abs_in) = scoped(ctx, async {
        (user_path(Path::new("a.jpg")), user_path(abs))
    })
    .await;
    assert_eq!(rel, Path::new("/home/u/shots/a.jpg"));
    assert_eq!(abs_in, abs);
}

#[tokio::test(flavor = "multi_thread")]
async fn blocking_read_uses_client_stdin_in_scope() {
    let (out_tx, _out_rx) = mpsc::unbounded_channel();
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    in_tx.send(b"yes\n".to_vec()).unwrap();
    let ctx = OutputCtx::new(out_tx, in_rx, None);
    let line = scoped(ctx, async { read_line_opt_blocking().unwrap() }).await;
    assert_eq!(line.as_deref(), Some("yes"));
}

#[test]
fn tty_prompt_skips_pump_until_started() {
    // Standalone TTY: one-shot read, no pump.
    assert!(direct_tty_read(false, true, false));
    // Pump running: must share its buffer.
    assert!(!direct_tty_read(false, true, true));
    // Piped stdin keeps the shared pump.
    assert!(!direct_tty_read(false, false, false));
    // Daemon context reads client stdin.
    assert!(!direct_tty_read(true, true, false));
}
