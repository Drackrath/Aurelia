use aurelia::launch::pipeline::PipelineContext;
use aurelia::launch::validators::overrides::OverrideConflictValidator;
use aurelia::launch::validators::LaunchValidator;
use aurelia::models::UserAppConfig;

#[tokio::test]
async fn test_validator_detects_dxvk_conflict() {
    let mut ctx = PipelineContext::new(1);
    let mut user_config = UserAppConfig::default();
    user_config.graphics_layers.dxvk_enabled = true;
    user_config.env_variables.insert(
        "WINEDLLOVERRIDES".to_string(),
        "d3d11=b".to_string()
    );
    ctx.user_config = Some(user_config);

    let validator = OverrideConflictValidator;
    validator.validate(&mut ctx);

    assert_eq!(ctx.warnings.len(), 1);
    assert_eq!(ctx.warnings[0].code, "OVERRIDE_CONFLICT_DXVK");
    assert!(ctx.warnings[0].message.contains("d3d11=b"));
}

#[tokio::test]
async fn test_validator_detects_vkd3d_conflict() {
    let mut ctx = PipelineContext::new(2);
    let mut user_config = UserAppConfig::default();
    user_config.graphics_layers.vkd3d_proton_enabled = true;
    user_config.env_variables.insert(
        "WINEDLLOVERRIDES".to_string(),
        "d3d12=b,n".to_string()
    );
    ctx.user_config = Some(user_config);

    let validator = OverrideConflictValidator;
    validator.validate(&mut ctx);

    assert_eq!(ctx.warnings.len(), 1);
    assert_eq!(ctx.warnings[0].code, "OVERRIDE_CONFLICT_VKD3D");
}

#[tokio::test]
async fn test_validator_no_conflict_when_layers_disabled() {
    let mut ctx = PipelineContext::new(3);
    let mut user_config = UserAppConfig::default();
    user_config.graphics_layers.dxvk_enabled = false;
    user_config.env_variables.insert(
        "WINEDLLOVERRIDES".to_string(),
        "d3d11=b".to_string()
    );
    ctx.user_config = Some(user_config);

    let validator = OverrideConflictValidator;
    validator.validate(&mut ctx);

    assert_eq!(ctx.warnings.len(), 0);
}

#[tokio::test]
async fn test_validator_detects_contradiction() {
    let mut ctx = PipelineContext::new(4);
    let mut user_config = UserAppConfig::default();
    user_config.env_variables.insert(
        "WINEDLLOVERRIDES".to_string(),
        "d3d11=b;d3d11=n".to_string()
    );
    ctx.user_config = Some(user_config);

    let validator = OverrideConflictValidator;
    validator.validate(&mut ctx);

    assert_eq!(ctx.warnings.len(), 1);
    assert_eq!(ctx.warnings[0].code, "OVERRIDE_CONTRADICTION");
    assert!(ctx.warnings[0].message.contains("d3d11"));
}
