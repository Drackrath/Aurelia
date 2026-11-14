//! Private `~/.steam` overlay per launch.

use crate::infra::runners::CommandSpec;
use crate::launch::pipeline::{LaunchError, LaunchErrorKind};
use std::path::{Path, PathBuf};

/// Overlaid entries, hidden from the mirror.
const OVERLAID: [&str; 4] = ["sdk32", "sdk64", "steam.pid", "registry.vdf"];

/// Env var the stand-in reads.
pub const DAEMON_SOCKET_ENV: &str = "AURELIA_DAEMON_SOCKET";

/// Env var naming the stand-in log.
pub const LOG_ENV: &str = "AURELIA_STEAMWORKS_LOG";

/// Prefix of stand-in log lines.
const LOG_PREFIX: &str = "[aurelia-steamworks] ";

/// Paths pressure-vessel shares read-write.
const PV_FILESYSTEMS_RW: &str = "PRESSURE_VESSEL_FILESYSTEMS_RW";

static DAEMON_SOCKET: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Record the daemon's socket path.
pub fn set_daemon_socket(path: PathBuf) {
    let _ = DAEMON_SOCKET.set(path);
}

/// Daemon socket for stand-in launches.
pub fn daemon_socket() -> Option<PathBuf> {
    DAEMON_SOCKET.get().cloned()
}

/// Writes pid files, execs the game.
const WRAPPER_SCRIPT: &str = r#"d=$1; head=$2; tail=$3; shift 3
printf '%s\n' "$$" > "$d/steam.pid" || exit 127
printf '%s%s%s' "$head" "$$" "$tail" > "$d/registry.vdf" || exit 127
exec "$@""#;

/// Stand-in libraries per architecture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientLibs {
    pub sdk64: PathBuf,
    pub sdk32: Option<PathBuf>,
}

/// How real entries are mirrored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirrorKind {
    /// Recreate the symlink as is.
    Symlink(PathBuf),
    /// Bind the real file or directory.
    Bind,
}

/// One mirrored `~/.steam` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorEntry {
    pub name: String,
    pub kind: MirrorKind,
}

/// Everything needed to wrap one launch.
#[derive(Debug, Clone)]
pub struct OverlayPlan {
    pub bwrap: PathBuf,
    pub home: PathBuf,
    pub libs: ClientLibs,
    pub mirror: Vec<MirrorEntry>,
    pub app_id: u32,
    pub account_id: Option<u32>,
    pub daemon_socket: Option<PathBuf>,
    pub log_path: Option<PathBuf>,
    /// Read-only file binds: source, target.
    pub dll_binds: Vec<(PathBuf, PathBuf)>,
}

/// Steam folder inside a Wine prefix.
const PREFIX_STEAM_DIR: &str = "drive_c/Program Files (x86)/Steam";

/// Prefix Proton runs the game in.
pub fn game_prefix(env: &std::collections::HashMap<String, String>) -> Option<PathBuf> {
    let compat = env
        .get("STEAM_COMPAT_DATA_PATH")
        .map(|p| Path::new(p).join("pfx"))
        .filter(|p| p.is_dir());
    compat.or_else(|| {
        env.get("WINEPREFIX")
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
    })
}

/// Proton root from env or argv.
pub fn runner_root(spec: &CommandSpec) -> Option<PathBuf> {
    if let Some(path) = spec.env.get("PROTONPATH").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    std::iter::once(spec.program.to_string_lossy().into_owned())
        .chain(spec.args.iter().cloned())
        .map(PathBuf::from)
        .find(|p| p.file_name().is_some_and(|n| n == "proton"))
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

/// Proton's shim: legacycompat, else lsteamclient.
pub fn shim_sources(host_steam: Option<&Path>, runner: Option<&Path>) -> [Option<PathBuf>; 2] {
    let pick = |legacy: &str, pe_dir: &str| {
        let valve = host_steam.map(|s| s.join("legacycompat").join(legacy));
        let builtin = runner.map(|r| {
            r.join("files/lib/wine")
                .join(pe_dir)
                .join("lsteamclient.dll")
        });
        [valve, builtin]
            .into_iter()
            .flatten()
            .find(|p| p.is_file())
            .map(|p| p.canonicalize().unwrap_or(p))
    };
    [
        pick("steamclient64.dll", "x86_64-windows"),
        pick("steamclient.dll", "i386-windows"),
    ]
}

/// Prefix DLLs to shadow, existing only.
pub fn prefix_dll_binds(prefix: &Path, shims: &[Option<PathBuf>; 2]) -> Vec<(PathBuf, PathBuf)> {
    let steam = prefix.join(PREFIX_STEAM_DIR);
    let targets: [(&str, &Option<PathBuf>); 3] = [
        ("steamclient64.dll", &shims[0]),
        ("steamclient64_aurelia.dll", &shims[0]),
        ("steamclient.dll", &shims[1]),
    ];
    targets
        .into_iter()
        .filter_map(|(name, shim)| {
            let target = steam.join(name);
            // Never create files on disk.
            let shim = shim.as_ref()?;
            (target.is_file() && target.canonicalize().ok() != shim.canonicalize().ok())
                .then(|| (shim.clone(), target))
        })
        .collect()
}

/// Default stand-in install directory.
pub fn default_client_dir() -> Option<PathBuf> {
    crate::core::config::data_dir()
        .ok()
        .map(|d| d.join("steamworks"))
}

/// Library file, or `sdk64`/`sdk32` directory.
pub fn resolve_client_libs(path: &Path) -> Result<ClientLibs, LaunchError> {
    if path.is_file() {
        return Ok(ClientLibs {
            sdk64: path.to_path_buf(),
            sdk32: None,
        });
    }
    let sdk64 = path.join("sdk64/steamclient.so");
    if !sdk64.is_file() {
        return Err(LaunchError::new(
            LaunchErrorKind::Dependency,
            format!(
                "steamworks_client is on but {} is missing; build aurelia-steamworks \
                 and install its libsteamclient.so there, or set the path with \
                 `aurelia config steamworks-client on --path <file|dir>`",
                sdk64.display()
            ),
        ));
    }
    let sdk32 = Some(path.join("sdk32/steamclient.so")).filter(|p| p.is_file());
    Ok(ClientLibs { sdk64, sdk32 })
}

/// List real `~/.steam` entries to mirror.
pub fn scan_steam_dir(steam_dir: &Path) -> Vec<MirrorEntry> {
    let Ok(entries) = std::fs::read_dir(steam_dir) else {
        return Vec::new();
    };
    let mut mirror: Vec<MirrorEntry> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if OVERLAID.contains(&name.as_str()) {
                return None;
            }
            let kind = match std::fs::read_link(e.path()) {
                Ok(target) => MirrorKind::Symlink(target),
                Err(_) => MirrorKind::Bind,
            };
            Some(MirrorEntry { name, kind })
        })
        .collect();
    mirror.sort_by(|a, b| a.name.cmp(&b.name));
    mirror
}

/// Find `bwrap` on `PATH`.
pub fn find_bwrap() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("bwrap"))
        .find(|p| p.is_file())
}

/// Escape a VDF string value.
fn vdf_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `registry.vdf` text around the pid.
pub fn registry_vdf_parts(steam_dir: &Path, account_id: Option<u32>) -> (String, String) {
    let lib = |arch: &str| {
        vdf_escape(
            &steam_dir
                .join(arch)
                .join("steamclient.so")
                .to_string_lossy(),
        )
    };
    let head =
        "\"Registry\"\n{\n\t\"HKCU\"\n\t{\n\t\t\"Software\"\n\t\t{\n\t\t\t\"Valve\"\n\t\t\t{\n\
                \t\t\t\t\"Steam\"\n\t\t\t\t{\n\t\t\t\t\t\"ActiveProcess\"\n\t\t\t\t\t{\n\
                \t\t\t\t\t\t\"pid\"\t\t\""
            .to_string();
    let mut tail = format!(
        "\"\n\t\t\t\t\t\t\"SteamClientDll\"\t\t\"{}\"\n\t\t\t\t\t\t\"SteamClientDll64\"\t\t\"{}\"\n",
        lib("sdk32"),
        lib("sdk64"),
    );
    if let Some(account) = account_id {
        tail.push_str(&format!("\t\t\t\t\t\t\"ActiveUser\"\t\t\"{account}\"\n"));
    }
    tail.push_str("\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n}\n");
    (head, tail)
}

/// `bwrap` arguments that build the overlay.
pub fn bwrap_args(plan: &OverlayPlan, cwd: Option<&Path>) -> Vec<String> {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let steam = plan.home.join(".steam");
    let mut args: Vec<String> = vec!["--dev-bind".into(), "/".into(), "/".into()];
    args.extend(["--tmpfs".into(), s(&steam)]);
    for entry in &plan.mirror {
        let dest = s(&steam.join(&entry.name));
        match &entry.kind {
            MirrorKind::Symlink(target) => args.extend(["--symlink".into(), s(target), dest]),
            MirrorKind::Bind => {
                let src = dest.clone();
                args.extend(["--bind".into(), src, dest]);
            }
        }
    }
    let mut lib = |arch: &str, so: &Path| {
        let dir = steam.join(arch);
        args.extend(["--dir".into(), s(&dir)]);
        args.extend(["--ro-bind".into(), s(so), s(&dir.join("steamclient.so"))]);
    };
    lib("sdk64", &plan.libs.sdk64);
    if let Some(so32) = &plan.libs.sdk32 {
        lib("sdk32", so32);
    }
    for (src, target) in &plan.dll_binds {
        args.extend(["--ro-bind".into(), s(src), s(target)]);
    }
    if let Some(cwd) = cwd {
        args.extend(["--chdir".into(), s(cwd)]);
    }
    args
}

/// Run `spec` inside the overlay.
pub fn wrap_command(spec: &CommandSpec, plan: &OverlayPlan) -> CommandSpec {
    let steam = plan.home.join(".steam");
    let (head, tail) = registry_vdf_parts(&steam, plan.account_id);
    let mut args = bwrap_args(plan, spec.cwd.as_deref());
    args.extend([
        "--".into(),
        "/bin/sh".into(),
        "-c".into(),
        WRAPPER_SCRIPT.into(),
        "aurelia-steamworks".into(),
        steam.to_string_lossy().into_owned(),
        head,
        tail,
        spec.program.to_string_lossy().into_owned(),
    ]);
    args.extend(spec.args.iter().cloned());

    let mut env = spec.env.clone();
    let id = plan.app_id.to_string();
    env.entry("SteamAppId".into()).or_insert_with(|| id.clone());
    env.entry("SteamGameId".into()).or_insert(id);
    // Proton must keep lsteamclient enabled.
    env.remove("PROTON_DISABLE_LSTEAMCLIENT");
    if let Some(socket) = &plan.daemon_socket {
        add_daemon_socket(&mut env, socket);
    }
    if let Some(log) = &plan.log_path {
        add_log_path(&mut env, log);
    }

    CommandSpec {
        program: plan.bwrap.clone(),
        args,
        cwd: spec.cwd.clone(),
        env,
    }
}

/// Point the stand-in at the daemon.
///
/// Also shares it into pressure-vessel.
pub fn add_daemon_socket(env: &mut std::collections::HashMap<String, String>, socket: &Path) {
    env.insert(
        DAEMON_SOCKET_ENV.into(),
        socket.to_string_lossy().into_owned(),
    );
    share_rw(env, socket);
}

/// Point the stand-in log at `log`.
pub fn add_log_path(env: &mut std::collections::HashMap<String, String>, log: &Path) {
    env.insert(LOG_ENV.into(), log.to_string_lossy().into_owned());
    // Share directory; file may be new.
    share_rw(env, log.parent().unwrap_or(log));
}

/// Share `path` read-write into pressure-vessel.
fn share_rw(env: &mut std::collections::HashMap<String, String>, path: &Path) {
    let path = path.to_string_lossy().into_owned();
    let shared = env.entry(PV_FILESYSTEMS_RW.into()).or_default();
    if !shared.split(':').any(|p| p == path) {
        if !shared.is_empty() {
            shared.push(':');
        }
        shared.push_str(&path);
    }
}

/// Deduplicated findings from a stand-in log.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LogSummary {
    pub missing: Vec<String>,
    pub networking: Vec<String>,
}

impl LogSummary {
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.networking.is_empty()
    }

    /// User-facing lines, log path last.
    pub fn lines(&self, log: &Path) -> Vec<String> {
        let mut out = Vec::new();
        if !self.missing.is_empty() {
            out.push(format!(
                "stand-in steamclient.so: game asked for missing interfaces: {}",
                self.missing.join(", ")
            ));
        }
        if !self.networking.is_empty() {
            out.push(format!(
                "stand-in steamclient.so: game used Steam networking ({}); \
                 multiplayer may not have connected",
                self.networking.join(", ")
            ));
        }
        if !out.is_empty() {
            out.push(format!("stand-in log: {}", log.display()));
        }
        out
    }
}

/// Collect missing and networking interfaces.
pub fn summarize_log(text: &str) -> LogSummary {
    let mut summary = LogSummary::default();
    let push = |list: &mut Vec<String>, v: &str| {
        let v = v.trim();
        if !v.is_empty() && !list.iter().any(|x| x == v) {
            list.push(v.to_string());
        }
    };
    for line in text.lines() {
        let Some(msg) = line.strip_prefix(LOG_PREFIX) else {
            continue;
        };
        if let Some(rest) = msg.strip_prefix("missing interface: ") {
            push(&mut summary.missing, rest);
        } else if let Some(rest) = msg.strip_prefix("game reports missing interface: ") {
            let version = rest.split(" (pipe ").next().unwrap_or(rest);
            push(&mut summary.missing, version);
        } else if let Some((version, _)) = msg
            .strip_prefix("game created ")
            .and_then(|rest| rest.split_once(": Steam networking"))
        {
            push(&mut summary.networking, version);
        }
    }
    summary
}

/// Summary of a finished launch's log.
pub fn summarize_log_file(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .map(|text| summarize_log(&text).lines(log))
        .unwrap_or_default()
}

/// Build the plan for one launch.
pub fn plan_for(
    config: &crate::core::config::LauncherConfig,
    spec: &CommandSpec,
    app_id: u32,
    account_id: Option<u32>,
) -> Result<OverlayPlan, LaunchError> {
    let dir = config
        .steamworks_client_path
        .as_deref()
        .map(PathBuf::from)
        .or_else(default_client_dir)
        .ok_or_else(|| {
            LaunchError::new(
                LaunchErrorKind::Environment,
                "cannot locate the stand-in steamclient.so directory",
            )
        })?;
    let libs = resolve_client_libs(&dir)?;
    let bwrap = find_bwrap().ok_or_else(|| {
        LaunchError::new(
            LaunchErrorKind::Dependency,
            "steamworks_client is on but `bwrap` (bubblewrap) is not installed",
        )
    })?;
    let home = match spec.env.get("HOME").filter(|h| !h.is_empty()) {
        Some(home) => PathBuf::from(home),
        None => crate::core::config::home_dir().map_err(|e| {
            LaunchError::new(LaunchErrorKind::Environment, "HOME is not set").with_source(e)
        })?,
    };
    let steam = home.join(".steam");
    if !steam.exists() {
        // Mount point only; stays empty.
        std::fs::create_dir_all(&steam).map_err(|e| {
            LaunchError::new(
                LaunchErrorKind::Permission,
                format!("cannot create mount point {}", steam.display()),
            )
            .with_source(e.into())
        })?;
    }
    let dll_binds = game_prefix(&spec.env)
        .map(|prefix| {
            let host = crate::core::config::detect_steam_path();
            let shims = shim_sources(host.as_deref(), runner_root(spec).as_deref());
            prefix_dll_binds(&prefix, &shims)
        })
        .unwrap_or_default();
    for (src, target) in &dll_binds {
        tracing::info!(
            "stand-in: {} shadowed by {} for this launch",
            target.display(),
            src.display()
        );
    }
    Ok(OverlayPlan {
        bwrap,
        home,
        libs,
        mirror: scan_steam_dir(&steam),
        app_id,
        account_id,
        daemon_socket: daemon_socket(),
        log_path: None,
        dll_binds,
    })
}

/// Whether this runner gets the overlay.
pub fn runner_supported(name: &str) -> bool {
    matches!(name, "Native" | "Wine-TKG")
}
