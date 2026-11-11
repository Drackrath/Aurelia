//! Anti-cheat runtime install before launch.
use super::*;
use crate::compat::anticheat::{self, AntiCheat};

/// Cap on a pre-launch install.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

impl SteamClient {
    /// Install missing runtimes; best-effort.
    pub(crate) async fn ensure_anticheat_runtimes(&self, app: &LibraryGame) {
        let Some(dir) = app.install_path.clone() else {
            return;
        };
        let kinds = tokio::task::spawn_blocking(move || anticheat::detect(Path::new(&dir)))
            .await
            .unwrap_or_default();
        if kinds.is_empty() {
            return;
        }
        let installed = anticheat::installed_runtimes().await;
        for kind in kinds {
            if installed.contains_key(&kind.runtime_app_id()) {
                continue;
            }
            if self.is_offline() {
                tracing::warn!(
                    appid = app.app_id,
                    "{} is missing and Aurelia is offline; the anti-cheat may refuse to start",
                    kind.runtime_name()
                );
                continue;
            }
            tracing::info!(
                appid = app.app_id,
                "Installing {} (app {}) for this game's anti-cheat",
                kind.runtime_name(),
                kind.runtime_app_id()
            );
            let install =
                tokio::time::timeout(INSTALL_TIMEOUT, self.install_anticheat_runtime(kind));
            match install.await.unwrap_or_else(|_| Err(anyhow!("timed out"))) {
                Ok(()) => tracing::info!("Installed {}", kind.runtime_name()),
                Err(e) => tracing::warn!(
                    appid = app.app_id,
                    "Could not install {}; launching anyway: {e:#}",
                    kind.runtime_name()
                ),
            }
        }
    }

    async fn install_anticheat_runtime(&self, kind: AntiCheat) -> Result<()> {
        let state = Arc::new(std::sync::RwLock::new(
            crate::core::models::DownloadState::default(),
        ));
        let mut rx = self
            .install_game(
                kind.runtime_app_id(),
                DepotPlatform::Linux,
                InstallOptions::default(),
                state,
            )
            .await?;
        while let Some(progress) = rx.recv().await {
            match progress.state {
                DownloadProgressState::Completed => return Ok(()),
                DownloadProgressState::Failed => bail!("{}", progress.current_file),
                _ => {}
            }
        }
        bail!("download ended before completing")
    }
}
