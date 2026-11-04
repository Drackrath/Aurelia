//! Vulkan shader pre-caching via `fossilize_replay`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};

/// Steam's per-game pipeline folder.
const FOZ_DIR: &str = "fozpipelinesv6";
/// Aurelia's record of the last replay.
const MARKER: &str = "aurelia_replay.json";

/// What the replay reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShaderEvent {
    /// Replay starting over `databases`.
    Started { databases: usize },
    /// `done` of `total` entries handled.
    Progress { done: u64, total: u64 },
    /// Replay ran to the end.
    Finished { done: u64, total: u64 },
    /// Nothing to do, and why.
    Skipped(String),
}

/// `config shader-precache` modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShaderPrecache {
    /// Replay before launch, like Steam.
    #[default]
    Auto,
    /// Leave shaders to the game.
    Off,
}

/// Callback for [`ShaderEvent`]s.
pub type ShaderProgressFn = Arc<dyn Fn(ShaderEvent) + Send + Sync>;

/// Per-launch shader options.
#[derive(Clone, Default)]
pub struct ShaderLaunch {
    /// Skip the replay this time.
    pub skip: bool,
    pub on_event: Option<ShaderProgressFn>,
}

/// `<steamapps>/shadercache/<appid>` for an install.
pub fn shader_cache_dir(install_dir: &Path, app_id: u32) -> Option<PathBuf> {
    // Installs live in `steamapps/common/<dir>`.
    let steamapps = install_dir.parent()?.parent()?;
    let named = steamapps
        .file_name()?
        .to_string_lossy()
        .eq_ignore_ascii_case("steamapps");
    named.then(|| steamapps.join("shadercache").join(app_id.to_string()))
}

/// Driver cache variables Steam sets.
pub fn cache_env(dir: &Path) -> Vec<(&'static str, String)> {
    let path = |p: &Path| p.to_string_lossy().into_owned();
    vec![
        ("__GL_SHADER_DISK_CACHE", "1".to_string()),
        ("__GL_SHADER_DISK_CACHE_PATH", path(&dir.join("nvidiav1"))),
        ("__GL_SHADER_DISK_CACHE_SKIP_CLEANUP", "1".to_string()),
        ("MESA_SHADER_CACHE_DIR", path(dir)),
        ("DXVK_STATE_CACHE_PATH", path(&dir.join("DXVK_state_cache"))),
    ]
}

/// Create the cache folders drivers expect.
pub fn ensure_cache_dirs(dir: &Path) -> std::io::Result<()> {
    // NVIDIA ignores a missing path.
    std::fs::create_dir_all(dir.join("nvidiav1"))?;
    std::fs::create_dir_all(dir.join("DXVK_state_cache"))
}

/// Pipeline databases worth replaying.
pub fn databases(dir: &Path) -> Vec<PathBuf> {
    let foz = dir.join(FOZ_DIR);
    let mut found = Vec::new();
    for name in ["steam_pipeline_cache.foz", "steamapp_pipeline_cache.foz"] {
        let p = foz.join(name);
        if p.is_file() {
            found.push(p);
        }
    }
    // Per-run recordings: `steamapprun_pipeline_cache.*/`.
    if let Ok(entries) = std::fs::read_dir(&foz) {
        let mut runs: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("steamapprun_pipeline_cache")
            })
            .map(|e| e.path().join("steam_pipeline_cache.foz"))
            .filter(|p| p.is_file())
            .collect();
        runs.sort();
        found.extend(runs);
    }
    found
}

/// The replay tool, if installed.
pub fn find_replayer(steam_roots: &[PathBuf]) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("AURELIA_FOSSILIZE_REPLAY").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    for root in steam_roots {
        let p = root.join("ubuntu12_64").join("fossilize_replay");
        if p.is_file() {
            return Some(p);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|d| ["fossilize_replay", "fossilize-replay"].map(|n| d.join(n)))
        .find(|p| p.is_file())
}

/// Host Steam install folders.
pub fn host_steam_roots() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let home = PathBuf::from(home);
    [".steam/steam", ".local/share/Steam", ".steam/root"]
        .iter()
        .map(|p| home.join(p))
        .filter(|p| p.is_dir())
        .collect()
}

/// Size and mtime of `path`.
fn stamp(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(format!("{}:{mtime}", meta.len()))
}

/// Changes when the Vulkan driver does.
pub fn driver_key() -> String {
    let mut parts = Vec::new();
    if let Ok(v) = std::fs::read_to_string("/proc/driver/nvidia/version") {
        parts.push(v.lines().next().unwrap_or_default().trim().to_string());
    }
    let mut icds: Vec<PathBuf> = ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d"]
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|rd| rd.flatten().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    icds.sort();
    for icd in icds {
        let lib = std::fs::read_to_string(&icd)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|v| v["ICD"]["library_path"].as_str().map(PathBuf::from));
        let lib_stamp = lib
            .as_deref()
            .and_then(resolve_library)
            .and_then(|p| stamp(&p));
        parts.push(format!(
            "{}={}",
            icd.display(),
            lib_stamp.or_else(|| stamp(&icd)).unwrap_or_default()
        ));
    }
    parts.join("|")
}

/// Find a bare ICD library name.
fn resolve_library(lib: &Path) -> Option<PathBuf> {
    if lib.is_absolute() {
        return Some(lib.to_path_buf());
    }
    ["/usr/lib", "/usr/lib64", "/usr/lib/x86_64-linux-gnu"]
        .iter()
        .map(|d| Path::new(d).join(lib))
        .find(|p| p.is_file())
}

/// Identity of a replay's inputs.
pub fn fingerprint(databases: &[PathBuf], driver: &str) -> String {
    let mut parts: Vec<String> = databases
        .iter()
        .map(|p| format!("{}={}", p.display(), stamp(p).unwrap_or_default()))
        .collect();
    parts.push(driver.to_string());
    parts.join("\n")
}

/// Whether `fingerprint` was already replayed.
pub fn is_up_to_date(dir: &Path, fingerprint: &str) -> bool {
    std::fs::read_to_string(dir.join(FOZ_DIR).join(MARKER))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .is_some_and(|v| v["fingerprint"] == fingerprint)
}

fn write_marker(dir: &Path, fingerprint: &str) -> std::io::Result<()> {
    let body = serde_json::json!({ "fingerprint": fingerprint }).to_string();
    std::fs::write(dir.join(FOZ_DIR).join(MARKER), body)
}

/// Parse fossilize's `Overall n / m` line.
pub fn parse_progress(line: &str) -> Option<(u64, u64)> {
    let rest = line.split("Overall").nth(1)?;
    let (done, total) = rest.split_once('/')?;
    Some((done.trim().parse().ok()?, total.trim().parse().ok()?))
}

/// Replay a game's pipelines into caches.
///
/// `env`: the game's cache variables.
pub async fn precache(
    dir: &Path,
    env: &HashMap<String, String>,
    force: bool,
    on_event: Option<&ShaderProgressFn>,
) -> Result<ShaderEvent> {
    let emit = |e: ShaderEvent| {
        if let Some(cb) = on_event {
            cb(e);
        }
    };
    let dbs = databases(dir);
    if dbs.is_empty() {
        return Ok(ShaderEvent::Skipped(
            "no Vulkan pipeline cache for this game".into(),
        ));
    }
    let Some(replayer) = find_replayer(&host_steam_roots()) else {
        return Ok(ShaderEvent::Skipped(
            "fossilize_replay not found (install Steam, or set AURELIA_FOSSILIZE_REPLAY)".into(),
        ));
    };
    let fp = fingerprint(&dbs, &driver_key());
    if !force && is_up_to_date(dir, &fp) {
        return Ok(ShaderEvent::Skipped("shaders already processed".into()));
    }
    ensure_cache_dirs(dir).context("failed creating shader cache folders")?;

    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut cmd = tokio::process::Command::new(&replayer);
    cmd.arg("--progress")
        .arg("--num-threads")
        .arg(threads.to_string())
        .args(&dbs)
        .envs(env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    tracing::info!(
        "replaying {} pipeline database(s) with {}",
        dbs.len(),
        replayer.display()
    );
    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed starting {}", replayer.display()))?;
    emit(ShaderEvent::Started {
        databases: dbs.len(),
    });

    // Read inline: keeps output context.
    let mut last = (0, 0);
    if let Some(stderr) = child.stderr.take() {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Some(line) = lines.next_line().await.unwrap_or(None) {
            if let Some(progress) = parse_progress(&line)
                && progress != last
            {
                last = progress;
                emit(ShaderEvent::Progress {
                    done: progress.0,
                    total: progress.1,
                });
            }
        }
    }
    let status = child.wait().await.context("fossilize_replay wait failed")?;
    if !status.success() {
        bail!("fossilize_replay exited with {status}");
    }
    if let Err(e) = write_marker(dir, &fp) {
        tracing::warn!("could not record shader replay: {e}");
    }
    let done = ShaderEvent::Finished {
        done: last.0,
        total: last.1,
    };
    emit(done.clone());
    Ok(done)
}

#[cfg(test)]
#[path = "shaders_tests.rs"]
mod tests;
