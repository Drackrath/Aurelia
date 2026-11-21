use super::super::*;

/// Build a Steam-style compressed cloud body: a zip holding the single real file.
fn zip_wrap(content: &[u8]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        w.start_file::<_, ()>("z", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut w, content).unwrap();
        w.finish().unwrap();
    }
    buf.into_inner()
}

fn sha1_hex(bytes: &[u8]) -> String {
    hex_lower(&Sha1::digest(bytes))
}

#[test]
fn compressed_download_is_unwrapped_to_the_real_file() {
    // The failing case: Steam ships the save inside a zip, so writing the body
    // verbatim leaves a zip on disk where the game expects its save. The reported
    // hash is over the *original* content, which is what identifies the unwrap.
    let content = b"GVAS real save payload".repeat(64);
    let body = zip_wrap(&content);
    assert_ne!(body, content, "test fixture must actually be wrapped");

    let out = maybe_decompress(
        body,
        29_112,
        content.len() as u32,
        Some(&sha1_hex(&content)),
        "%WinAppDataLocal%EXPEDITION_0.sav",
    );
    assert_eq!(out, content);
}

#[test]
fn uncompressed_download_passes_through_untouched() {
    let content = b"plain save bytes".to_vec();
    let out = maybe_decompress(
        content.clone(),
        content.len() as u32,
        content.len() as u32,
        Some(&sha1_hex(&content)),
        "save.dat",
    );
    assert_eq!(out, content);
}

#[test]
fn a_save_that_is_itself_a_zip_is_not_unwrapped() {
    // Some games store saves as zip archives. Steam's hash identifies the file as
    // already-correct, so it must be written as received rather than unpacked.
    let content = zip_wrap(b"inner game data");
    let out = maybe_decompress(
        content.clone(),
        content.len() as u32,
        content.len() as u32,
        Some(&sha1_hex(&content)),
        "archive.sav",
    );
    assert_eq!(out, content, "a genuine zip save must survive intact");
}

#[test]
fn decompresses_on_size_mismatch_when_no_hash_is_known() {
    let content = b"payload without a reported hash".repeat(32);
    let body = zip_wrap(&content);
    let out = maybe_decompress(body, 100, content.len() as u32, None, "save.dat");
    assert_eq!(out, content);
}

#[test]
fn oversized_cloud_unzip_keeps_raw_body() {
    let body = zip_wrap(&[7u8; 65]);
    assert_eq!(unzip_capped(&body, None, 65).map(|v| v.len()), Some(65));
    assert_eq!(unzip_capped(&body, None, 64), None);
}

#[test]
fn huge_size_hint_is_not_preallocated() {
    let body = zip_wrap(b"save");
    assert_eq!(
        unzip_single_entry(&body, Some(u32::MAX)).as_deref(),
        Some(&b"save"[..])
    );
}

#[test]
fn cloud_body_limit_follows_announced_size() {
    assert_eq!(cloud_body_limit(1000, 5000), 1000 + CLOUD_BODY_SLACK);
    assert_eq!(cloud_body_limit(0, 5000), 5000 + CLOUD_BODY_SLACK);
    assert_eq!(cloud_body_limit(0, 0), MAX_CLOUD_UNZIPPED);
    assert_eq!(cloud_body_limit(u32::MAX, 0), MAX_CLOUD_UNZIPPED);
}

#[test]
fn push_capped_accepts_exact_and_refuses_over() {
    let mut body = Vec::new();
    push_capped(&mut body, &[1; 6], 10).unwrap();
    push_capped(&mut body, &[2; 4], 10).unwrap();
    assert_eq!(body.len(), 10);
    assert!(push_capped(&mut body, &[3], 10).is_err());
    assert_eq!(body.len(), 10);
}

// One-shot HTTP stub on localhost.
async fn serve_once(body: Vec<u8>, content_length: bool) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut req = [0u8; 1024];
        let _ = sock.read(&mut req).await;
        let head = if content_length {
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len())
        } else {
            "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_string()
        };
        let _ = sock.write_all(head.as_bytes()).await;
        let _ = sock.write_all(&body).await;
        let _ = sock.shutdown().await;
    });
    format!("http://{addr}/")
}

#[tokio::test]
async fn read_body_capped_enforces_the_limit() {
    let client = reqwest::Client::new();
    for content_length in [true, false] {
        let url = serve_once(vec![7; 100], content_length).await;
        let resp = client.get(&url).send().await.unwrap();
        assert_eq!(read_body_capped(resp, 100).await.unwrap().len(), 100);

        let url = serve_once(vec![7; 101], content_length).await;
        let resp = client.get(&url).send().await.unwrap();
        assert!(read_body_capped(resp, 100).await.is_err());
    }
}
