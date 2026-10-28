use super::*;

fn touch(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn cache_dir_sits_beside_common() {
    let install = Path::new("/lib/steamapps/common/No Man's Sky");
    assert_eq!(
        shader_cache_dir(install, 275850),
        Some(PathBuf::from("/lib/steamapps/shadercache/275850"))
    );
    // Not a Steam library layout.
    assert_eq!(shader_cache_dir(Path::new("/games/nms"), 1), None);
}

#[test]
fn env_points_drivers_into_the_cache() {
    let env: HashMap<_, _> = cache_env(Path::new("/c/275850")).into_iter().collect();
    assert_eq!(env["__GL_SHADER_DISK_CACHE_PATH"], "/c/275850/nvidiav1");
    assert_eq!(env["MESA_SHADER_CACHE_DIR"], "/c/275850");
    assert_eq!(env["DXVK_STATE_CACHE_PATH"], "/c/275850/DXVK_state_cache");
    assert_eq!(env["__GL_SHADER_DISK_CACHE"], "1");
}

#[test]
fn finds_steam_and_run_databases_only() {
    let tmp = tempfile::tempdir().unwrap();
    let foz = tmp.path().join(FOZ_DIR);
    touch(&foz.join("steam_pipeline_cache.foz"), b"a");
    touch(&foz.join("steamapp_pipeline_cache.foz"), b"b");
    touch(
        &foz.join("steamapprun_pipeline_cache.923f/steam_pipeline_cache.foz"),
        b"c",
    );
    // Not replay inputs.
    touch(&foz.join("steam_pipeline_cache_whitelist.foz"), b"w");
    touch(&foz.join("replay_cache.0453.foz"), b"r");
    touch(
        &tmp.path()
            .join("downloads")
            .join(FOZ_DIR)
            .join("steam_pipeline_cache.foz"),
        b"d",
    );
    let names: Vec<String> = databases(tmp.path())
        .iter()
        .map(|p| p.strip_prefix(&foz).unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        [
            "steam_pipeline_cache.foz",
            "steamapp_pipeline_cache.foz",
            "steamapprun_pipeline_cache.923f/steam_pipeline_cache.foz"
        ]
    );
    assert!(databases(&tmp.path().join("missing")).is_empty());
}

#[test]
fn parses_overall_progress() {
    assert_eq!(
        parse_progress("Fossilize INFO:    Overall 24161 / 574464"),
        Some((24161, 574464))
    );
    assert_eq!(
        parse_progress("Fossilize INFO:    Overall 0 / 4"),
        Some((0, 4))
    );
    assert_eq!(
        parse_progress("Fossilize INFO:    Compile graphics 3099 / 568987"),
        None
    );
    assert_eq!(parse_progress("Fossilize INFO: ================="), None);
}

#[test]
fn marker_tracks_databases_and_driver() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join(FOZ_DIR).join("steam_pipeline_cache.foz");
    touch(&db, b"pipelines");
    let fp = fingerprint(std::slice::from_ref(&db), "driver-1");
    assert!(!is_up_to_date(tmp.path(), &fp));
    write_marker(tmp.path(), &fp).unwrap();
    assert!(is_up_to_date(tmp.path(), &fp));
    // A driver update invalidates it.
    assert!(!is_up_to_date(
        tmp.path(),
        &fingerprint(std::slice::from_ref(&db), "driver-2")
    ));
    // So does a grown database.
    touch(&db, b"more pipelines");
    assert!(!is_up_to_date(tmp.path(), &fingerprint(&[db], "driver-1")));
}

#[test]
fn replayer_override_wins() {
    // SAFETY: test-local variable.
    unsafe { std::env::set_var("AURELIA_FOSSILIZE_REPLAY", "/opt/fossilize_replay") };
    assert_eq!(
        find_replayer(&[]),
        Some(PathBuf::from("/opt/fossilize_replay"))
    );
    unsafe { std::env::remove_var("AURELIA_FOSSILIZE_REPLAY") };
    let tmp = tempfile::tempdir().unwrap();
    touch(&tmp.path().join("ubuntu12_64/fossilize_replay"), b"");
    assert_eq!(
        find_replayer(&[tmp.path().to_path_buf()]),
        Some(tmp.path().join("ubuntu12_64/fossilize_replay"))
    );
}

#[tokio::test]
async fn nothing_to_replay_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let out = precache(tmp.path(), &HashMap::new(), false, None)
        .await
        .unwrap();
    assert!(matches!(out, ShaderEvent::Skipped(_)));
}

// Real replay: `AURELIA_TEST_SHADER_DIR=<dir> cargo test -- --ignored`.
#[tokio::test]
#[ignore]
async fn replays_a_real_cache() {
    let dir = PathBuf::from(std::env::var("AURELIA_TEST_SHADER_DIR").expect("set the dir"));
    let env: HashMap<String, String> = cache_env(&dir)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = events.clone();
    let cb: ShaderProgressFn = Arc::new(move |e| sink.lock().unwrap().push(e));
    let first = precache(&dir, &env, true, Some(&cb)).await.unwrap();
    let ShaderEvent::Finished { done, total } = first else {
        panic!("{first:?}");
    };
    assert!(total > 0 && done == total, "{done}/{total}");
    let seen = events.lock().unwrap().clone();
    assert!(matches!(seen.first(), Some(ShaderEvent::Started { .. })));
    assert!(
        seen.iter()
            .any(|e| matches!(e, ShaderEvent::Progress { .. }))
    );
    assert!(
        dir.join("nvidiav1").read_dir().unwrap().next().is_some()
            || dir.join("mesa_shader_cache").exists()
            || dir.join("mesa_shader_cache_db").exists()
    );
    // Unchanged inputs: no second replay.
    let second = precache(&dir, &env, false, None).await.unwrap();
    assert_eq!(
        second,
        ShaderEvent::Skipped("shaders already processed".into())
    );
    println!("replayed {done}/{total}; events {}", seen.len());
}
