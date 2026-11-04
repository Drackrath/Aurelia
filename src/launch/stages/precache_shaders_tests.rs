use super::*;
use crate::infra::runners::CommandSpec;
use std::sync::{Arc, Mutex};

fn ctx_for(lib: &std::path::Path, mode: ShaderPrecache) -> PipelineContext {
    let mut ctx = PipelineContext::new(400);
    ctx.resolved_install_dir = Some(lib.join("steamapps/common/Portal"));
    let mut config = crate::core::config::LauncherConfig::default();
    config.shader_precache = mode;
    ctx.launcher_config = Some(config);
    ctx.command_spec = Some(CommandSpec {
        program: "game".into(),
        args: Vec::new(),
        cwd: None,
        env: Default::default(),
    });
    ctx
}

fn events(ctx: &mut PipelineContext) -> Arc<Mutex<Vec<ShaderEvent>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    ctx.on_shader_event = Some(Arc::new(move |e| sink.lock().unwrap().push(e)));
    seen
}

#[tokio::test]
async fn points_the_game_at_steams_caches() {
    let lib = tempfile::tempdir().unwrap();
    let mut ctx = ctx_for(lib.path(), ShaderPrecache::Auto);
    ctx.skip_shaders = true;
    let seen = events(&mut ctx);
    PrecacheShadersStage.execute(&mut ctx).await.unwrap();
    let cache = lib.path().join("steamapps/shadercache/400");
    let env = &ctx.command_spec.as_ref().unwrap().env;
    assert_eq!(
        env["__GL_SHADER_DISK_CACHE_PATH"],
        cache.join("nvidiav1").to_string_lossy()
    );
    assert_eq!(env["MESA_SHADER_CACHE_DIR"], cache.to_string_lossy());
    // NVIDIA needs the folder to exist.
    assert!(cache.join("nvidiav1").is_dir());
    assert_eq!(
        *seen.lock().unwrap(),
        [ShaderEvent::Skipped("--skip-shaders".into())]
    );
}

#[tokio::test]
async fn keeps_values_the_runner_set() {
    let lib = tempfile::tempdir().unwrap();
    let mut ctx = ctx_for(lib.path(), ShaderPrecache::Auto);
    ctx.skip_shaders = true;
    let spec = ctx.command_spec.as_mut().unwrap();
    spec.env
        .insert("MESA_SHADER_CACHE_DIR".into(), "/mine".into());
    PrecacheShadersStage.execute(&mut ctx).await.unwrap();
    let env = &ctx.command_spec.as_ref().unwrap().env;
    assert_eq!(env["MESA_SHADER_CACHE_DIR"], "/mine");
}

#[tokio::test]
async fn off_changes_nothing() {
    let lib = tempfile::tempdir().unwrap();
    let mut ctx = ctx_for(lib.path(), ShaderPrecache::Off);
    PrecacheShadersStage.execute(&mut ctx).await.unwrap();
    assert!(ctx.command_spec.as_ref().unwrap().env.is_empty());
    assert!(!lib.path().join("steamapps/shadercache").exists());
}

#[tokio::test]
async fn no_cache_is_reported_as_skipped() {
    let lib = tempfile::tempdir().unwrap();
    let mut ctx = ctx_for(lib.path(), ShaderPrecache::Auto);
    let seen = events(&mut ctx);
    PrecacheShadersStage.execute(&mut ctx).await.unwrap();
    let seen = seen.lock().unwrap();
    assert!(
        matches!(seen.as_slice(), [ShaderEvent::Skipped(r)] if r.contains("no Vulkan pipeline cache"))
    );
}

#[tokio::test]
async fn outside_a_steam_library_does_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut ctx = ctx_for(tmp.path(), ShaderPrecache::Auto);
    ctx.resolved_install_dir = Some(tmp.path().join("games/portal"));
    PrecacheShadersStage.execute(&mut ctx).await.unwrap();
    assert!(ctx.command_spec.as_ref().unwrap().env.is_empty());
}
