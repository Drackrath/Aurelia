use super::super::*;
use crate::core::models::{LibraryGame, UserAppConfig};
use crate::steam_client::{LaunchInfo, LaunchTarget};

#[tokio::test]
async fn native_runner_leaves_launch_options_to_their_stage() {
    let mut user_config = UserAppConfig::default();
    user_config.launch_options = "-fullscreen --skip-intro".to_string();

    let ctx = LaunchContext {
        app: LibraryGame {
            app_id: 123,
            name: "Test".to_string(),
            install_path: Some("/tmp/game".to_string()),
            is_installed: true,
            playtime_forever_minutes: None,
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
        },
        launch_info: LaunchInfo {
            app_id: 123,
            id: "0".into(),
            description: "Test".into(),
            executable: "game.bin".into(),
            arguments: "-v".into(),
            workingdir: None,
            target: LaunchTarget::NativeLinux,
        },
        launcher_config: crate::core::config::LauncherConfig::default(),
        user_config: Some(user_config),
        proton_path: None,
        steam_enabled: false,
        use_umu: false,
        umu_run: None,
        target_architecture: Default::default(),
        dll_resolutions: Vec::new(),
        game_fixups: Default::default(),
        verification_ptr: std::ptr::null_mut(),
    };

    let spec = NativeRunner.build_command(&ctx).await.unwrap();
    // UserLaunchOptionsStage adds them later.
    assert_eq!(spec.args, vec!["-v"]);
}
