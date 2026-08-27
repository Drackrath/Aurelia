use async_trait::async_trait;
use crate::launch::pipeline::{PipelineStage, PipelineContext, LaunchError, LaunchErrorKind};

pub struct ResolveGameStage;

#[async_trait]
impl PipelineStage for ResolveGameStage {
    fn name(&self) -> &str { "ResolveGame" }
    async fn execute(&self, ctx: &mut PipelineContext) -> std::result::Result<(), LaunchError> {
        let Some(app) = ctx.app.as_ref() else {
            // In the future, we might resolve the app here if only app_id is provided
            return Err(LaunchError::new(LaunchErrorKind::Validation, "App context missing in ResolveGameStage")
                .with_context("app_id", ctx.app_id.to_string()));
        };
        match app.install_path.as_deref() {
            None => Err(LaunchError::new(
                LaunchErrorKind::GameData,
                format!(
                    "{} is not installed on this machine. Run `aurelia install {}` first.",
                    app.name, ctx.app_id
                ),
            )),
            Some(p) if !std::path::Path::new(p).is_dir() => Err(LaunchError::new(
                LaunchErrorKind::GameData,
                format!(
                    "{} has install path {} but it does not exist on disk. \
                     Run `aurelia verify {}` or `aurelia install {}`.",
                    app.name, p, ctx.app_id, ctx.app_id
                ),
            )),
            _ => Ok(()),
        }
    }
}
