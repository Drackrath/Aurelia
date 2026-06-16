#[cfg(test)]
mod tests {
    use crate::infra::runners::{LaunchContext, CommandSpec, Runner, WineTkgRunner};
    use crate::models::LibraryGame;
    use crate::steam_client::{LaunchInfo, LaunchTarget};
    use crate::config::LauncherConfig;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn mock_context() -> LaunchContext {
        LaunchContext {
            app: LibraryGame {
                app_id: 123,
                name: "Test Game".to_string(),
                playtime_forever_minutes: None,
                is_installed: true,
                install_path: Some("/tmp/game".to_string()),
                local_manifest_ids: Default::default(),
                update_available: false,
                update_queued: false,
                active_branch: "public".to_string(),
                is_owned: true,
                is_family_shared: false,
                online_required: None,
                platform: None,
            },
            launch_info: LaunchInfo {
                app_id: 123,
                id: "0".to_string(),
                description: "Test".to_string(),
                executable: "game.exe".to_string(),
                arguments: "-v".to_string(),
                workingdir: None,
                target: LaunchTarget::WindowsProton,
            },
            launcher_config: LauncherConfig::default(),
            user_config: None,
            proton_path: Some("/tmp/proton".to_string()),
            steam_enabled: false,
            target_architecture: crate::models::ExecutableArchitecture::X86_64,
            dll_resolutions: Vec::new(),
            verification_ptr: std::ptr::null_mut(),
        }
    }

    #[test]
    fn test_launch_context_construction() {
        let ctx = mock_context();
        assert_eq!(ctx.app.app_id, 123);
    }

    #[test]
    fn test_command_spec_construction() {
        let spec = CommandSpec {
            program: PathBuf::from("/usr/bin/wine"),
            args: vec!["game.exe".to_string()],
            env: HashMap::from([("WINEPREFIX".to_string(), "/tmp/prefix".to_string())]),
            ..Default::default()
        };

        assert_eq!(spec.program, PathBuf::from("/usr/bin/wine"));
        assert_eq!(spec.args[0], "game.exe");
        assert_eq!(spec.env.get("WINEPREFIX").unwrap(), "/tmp/prefix");
    }

    #[tokio::test]
    async fn test_wine_tkg_runner_graphics_policy_autodetect() {
        use crate::models::{GraphicsBackendPolicy, UserAppConfig};
        use tempfile::tempdir;
        use std::fs;

        let tmp = tempdir().unwrap();
        let lib = tmp.path().join("library");
        let pfx = tmp.path().join("prefix");
        fs::create_dir_all(&lib).unwrap();
        fs::create_dir_all(&pfx).unwrap();

        // Simulate DXVK existence in runner path
        let runner_path = tmp.path().join("proton_8");
        let dxvk_dll = runner_path.join("files/lib/wine/dxvk/d3d11.dll");
        fs::create_dir_all(dxvk_dll.parent().unwrap()).unwrap();
        fs::write(&dxvk_dll, "fake dll content").unwrap();

        let app = LibraryGame {
            app_id: 123,
            name: "Test".to_string(),
            install_path: Some(tmp.path().to_string_lossy().to_string()),
            is_installed: true,
            playtime_forever_minutes: None,
            active_branch: "public".to_string(),
            update_available: false,
            update_queued: false,
            local_manifest_ids: HashMap::new(),
            is_owned: true,
            is_family_shared: false,
            online_required: None,
            platform: None,
        };

        let mut config = LauncherConfig::default();
        config.steam_library_path = lib.to_string_lossy().to_string();

        let mut user_config = UserAppConfig::default();
        user_config.graphics_layers.graphics_backend_policy = GraphicsBackendPolicy::Auto;

        let ctx = LaunchContext {
            app,
            launch_info: LaunchInfo {
                app_id: 123,
                id: "0".into(),
                description: "Test".into(),
                executable: "test.exe".into(),
                arguments: "".into(),
                workingdir: None,
                target: LaunchTarget::WindowsProton,
            },
            launcher_config: config,
            user_config: Some(user_config.clone()),
            proton_path: Some(runner_path.to_string_lossy().to_string()),
            steam_enabled: false,
            target_architecture: crate::models::ExecutableArchitecture::X86_64,
            dll_resolutions: Vec::new(),
            verification_ptr: std::ptr::null_mut(),
        };

        let runner = WineTkgRunner;
        let env = runner.build_env(&ctx).await.unwrap();

        // Auto is now CONSERVATIVE: it should NOT have enabled DXVK even if simulated on disk
        let overrides = env.get("WINEDLLOVERRIDES").unwrap();
        assert!(!overrides.contains("d3d11=n,b"));

        // Explicitly requesting DXVK should still enable it
        let mut user_config_dxvk = user_config.clone();
        user_config_dxvk.graphics_layers.graphics_backend_policy = GraphicsBackendPolicy::DXVK;
        let mut ctx_dxvk = ctx.clone();
        ctx_dxvk.user_config = Some(user_config_dxvk);
        let env_dxvk = runner.build_env(&ctx_dxvk).await.unwrap();
        let overrides_dxvk = env_dxvk.get("WINEDLLOVERRIDES").unwrap();
        // Strict mode uses 'n', not 'n,b'
        assert!(overrides_dxvk.contains("d3d11=n"));
        assert!(!overrides_dxvk.contains("d3d11=n,b"));
    }

    #[tokio::test]
    async fn test_wine_tkg_runner_stubs() {
        // We can only test stubs that don't hit the filesystem hard or expect real Proton/Wine
        let runner = WineTkgRunner;
        let ctx = mock_context();

        // build_env should succeed without real filesystem
        let env = runner.build_env(&ctx).await.unwrap();
        assert_eq!(env.get("SteamAppId").unwrap(), "123");
    }
}
