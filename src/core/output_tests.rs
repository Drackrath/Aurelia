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
