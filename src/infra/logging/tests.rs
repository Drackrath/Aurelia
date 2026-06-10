#[cfg(test)]
mod tests {
    use crate::infra::logging::{LaunchSessionId, LaunchSession, EventLogger, LogLevel, LogEvent};
    use std::path::PathBuf;
    use std::collections::{HashSet, HashMap};
    use tempfile::tempdir;

    #[test]
    fn test_launch_session_id_uniqueness() {
        let mut ids = HashSet::new();
        for _ in 0..100 {
            let id = LaunchSessionId::generate();
            assert!(ids.insert(id), "Duplicate session ID generated!");
        }
    }

    #[test]
    fn test_launch_session_path_generation() {
        let temp = PathBuf::from("/tmp/aurelia_logs");
        let session = LaunchSession::new(&temp);

        let id_str = session.id.to_string();
        assert!(session.log_dir.ends_with(&id_str));
        assert!(session.event_log_path().ends_with(format!("{}/events.jsonl", id_str)));
        assert!(session.stdout_path().ends_with(format!("{}/stdout.log", id_str)));
        assert!(session.stderr_path().ends_with(format!("{}/stderr.log", id_str)));
    }

    #[test]
    fn test_event_logger_append() {
        let dir = tempdir().expect("Failed to create temp dir");
        let session = LaunchSession::new(dir.path());
        let logger = EventLogger::new(&session).expect("Failed to create logger");

        logger.log(LogLevel::Info, "test_event", "Test message".to_string(), Some("Init".to_string()), HashMap::new())
            .expect("Failed to log event");

        let content = std::fs::read_to_string(session.event_log_path())
            .expect("Failed to read log file");

        let event: LogEvent = serde_json::from_str(&content)
            .expect("Failed to parse log event");

        assert_eq!(event.message, "Test message");
        assert_eq!(event.stage, Some("Init".to_string()));
        match event.level {
            LogLevel::Info => (),
            _ => panic!("Wrong log level"),
        }
    }

    #[test]
    fn test_event_redaction() {
        let dir = tempdir().expect("Failed to create temp dir");
        let session = LaunchSession::new(dir.path());
        let logger = EventLogger::new(&session).expect("Failed to create logger");

        let mut metadata = HashMap::new();
        metadata.insert("STEAM_TOKEN".to_string(), "secret123".to_string());
        metadata.insert("NORMAL_VAR".to_string(), "safe".to_string());

        logger.log(LogLevel::Info, "test", "msg".to_string(), None, metadata)
            .expect("Failed to log");

        let content = std::fs::read_to_string(session.event_log_path()).unwrap();
        let event: LogEvent = serde_json::from_str(&content).unwrap();

        assert_eq!(event.metadata.get("STEAM_TOKEN").unwrap(), "[REDACTED]");
        assert_eq!(event.metadata.get("NORMAL_VAR").unwrap(), "safe");
    }

    #[test]
    fn test_effective_env_redaction() {
        use crate::infra::logging::EffectiveEnv;
        let mut env_vars = HashMap::new();
        env_vars.insert("STEAM_TOKEN".to_string(), "secret123".to_string());
        env_vars.insert("REFRESH_TOKEN".to_string(), "abc-def".to_string());
        env_vars.insert("NORMAL_VAR".to_string(), "safe".to_string());

        let env = EffectiveEnv {
            runner_name: "Proton".to_string(),
            profile_id: None,
            profile_name: None,
            wine_dll_overrides: None,
            env_vars,
        };

        let dir = tempdir().unwrap();
        let session = LaunchSession::new(dir.path());
        session.write_effective_env(&env).unwrap();

        let content = std::fs::read_to_string(session.effective_env_path()).unwrap();
        let read_env: EffectiveEnv = serde_json::from_str(&content).unwrap();

        assert_eq!(read_env.env_vars.get("STEAM_TOKEN").unwrap(), "[REDACTED]");
        assert_eq!(read_env.env_vars.get("REFRESH_TOKEN").unwrap(), "[REDACTED]");
        assert_eq!(read_env.env_vars.get("NORMAL_VAR").unwrap(), "safe");
    }

    #[test]
    fn test_graphics_evidence_classification() {
        use crate::infra::logging::wine_capture::classify_graphics_evidence;

        let dxvk_line = "info:  DXVK: v2.3.1";
        assert!(classify_graphics_evidence(dxvk_line).unwrap().contains("DXVK Detected"));

        let vkd3d_line = "info:  VKD3D-Proton: v2.10";
        assert!(classify_graphics_evidence(vkd3d_line).unwrap().contains("VKD3D-Proton Detected"));

        let wined3d_line = "err:   wined3d: v8.0";
        assert!(classify_graphics_evidence(wined3d_line).unwrap().contains("WineD3D Fallback"));

        let random_line = "some random log";
        assert!(classify_graphics_evidence(random_line).is_none());
    }

    #[test]
    fn test_environment_sanity_checks() {
        use crate::infra::logging::check_environment_sanity;
        use crate::models::{UserAppConfig, GraphicsLayerConfig};

        // Case 1: Baseline with unexpected overrides
        let mut env = HashMap::new();
        env.insert("WINEDLLOVERRIDES".to_string(), "d3d11=n,b;dxgi=n,b".to_string());

        let mut config = UserAppConfig::default();
        config.graphics_layers = GraphicsLayerConfig {
            dxvk_enabled: false,
            vkd3d_proton_enabled: false,
            vkd3d_enabled: false,
            nvapi_enabled: true,
            graphics_backend_policy: crate::models::GraphicsBackendPolicy::WineD3D,
            d3d12_policy: crate::models::D3D12ProviderPolicy::Auto,
            use_symlinks_in_prefix: false,
            custom_dxvk_path: None,
            custom_vkd3d_path: None,
            custom_vkd3d_proton_path: None,
        };

        let warnings = check_environment_sanity(&env, "Wine", Some(&config));
        assert!(warnings.iter().any(|w| w.code == "SANITY_UNEXPECTED_OVERRIDE"));

        // Case 2: DXVK enabled but overrides missing
        let env = HashMap::new();
        let mut config = UserAppConfig::default();
        config.graphics_layers.dxvk_enabled = true;

        let warnings = check_environment_sanity(&env, "Wine", Some(&config));
        assert!(warnings.iter().any(|w| w.code == "SANITY_MISSING_OVERRIDES_ENV"));

        // Case 3: Proton missing data path
        let env = HashMap::new();
        let warnings = check_environment_sanity(&env, "Proton 8.0", None);
        assert!(warnings.iter().any(|w| w.code == "SANITY_MISSING_PROTON_DATA_PATH"));
    }
}
