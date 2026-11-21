use super::super::*;

#[tokio::test]
async fn test_legacy_path_blocks_windows_proton() {
    let client = SteamClient::new().unwrap();
    let app = LibraryGame {
        app_id: 123,
        name: "Test Game".to_string(),
        install_path: Some("/tmp/test_game".to_string()),
        is_installed: true,
        playtime_forever_minutes: Some(0),
        active_branch: "public".to_string(),
        update_available: false,
        steam_config_mismatch: false,
        update_queued: false,
        local_manifest_ids: HashMap::new(),
        is_owned: true,
        is_family_shared: false,
        online_required: None,
        platform: None,
        from_windows_steam: false,
    };
    let launch_info = LaunchInfo {
        app_id: 123,
        id: "0".to_string(),
        description: "Test".to_string(),
        executable: "test.exe".to_string(),
        arguments: "".to_string(),
        workingdir: None,
        target: LaunchTarget::WindowsProton,
    };
    let config = crate::core::config::LauncherConfig::default();

    let result = client
        .internal_legacy_launch_adhoc(&app, &launch_info, None, &config, None)
        .await;

    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Ad-hoc bypass is prohibited")
    );
}

#[tokio::test]
async fn test_pipeline_integration_scaffolding() {
    // Passing no app causes ResolveGame to fail early.
    let mut ctx = crate::launch::pipeline::PipelineContext::new(999999);
    let pipeline = crate::launch::pipeline::LaunchPipeline::with_default_stages();

    let result = pipeline.run(&mut ctx).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.stage_name, "ResolveGame");
    assert!(err.inner.to_string().contains("App context missing"));
}
