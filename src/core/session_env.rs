//! Per-request display session env.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::future::Future;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Variables that select the display session.
pub const SESSION_VARS: [&str; 7] = [
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "XDG_RUNTIME_DIR",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "DBUS_SESSION_BUS_ADDRESS",
];

/// Session display vars; absent means unset.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionEnv {
    vars: BTreeMap<String, String>,
}

impl SessionEnv {
    /// This process's session variables.
    pub fn capture() -> Self {
        Self::from_pairs(std::env::vars())
    }

    /// Keep only [`SESSION_VARS`], dropping empty values.
    pub fn from_pairs<K: AsRef<str>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        let vars = pairs
            .into_iter()
            .filter(|(k, _)| SESSION_VARS.contains(&k.as_ref()))
            .map(|(k, v)| (k.as_ref().to_string(), v.into()))
            .filter(|(_, v)| !v.is_empty())
            .collect();
        Self { vars }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }

    /// Set `key`; empty unsets.
    pub fn set(&mut self, key: &str, value: &str) {
        if value.is_empty() {
            self.vars.remove(key);
        } else {
            self.vars.insert(key.to_string(), value.to_string());
        }
    }

    pub fn vars(&self) -> &BTreeMap<String, String> {
        &self.vars
    }

    /// Set present vars, remove absent ones.
    pub fn apply(&self, cmd: &mut std::process::Command) {
        for key in SESSION_VARS {
            match self.vars.get(key) {
                Some(value) => cmd.env(key, value),
                None => cmd.env_remove(key),
            };
        }
    }

    /// Copy present vars into env.
    pub fn insert_into(&self, env: &mut HashMap<String, String>) {
        for (k, v) in &self.vars {
            env.insert(k.clone(), v.clone());
        }
    }

    /// Whether an X server looks reachable.
    pub fn has_x11(&self) -> bool {
        self.get("DISPLAY")
            .is_some_and(|d| x11_display_reachable(d, Path::new("/tmp/.X11-unix")))
    }

    /// Wayland present but no X server.
    pub fn is_wayland_only(&self) -> bool {
        self.get("WAYLAND_DISPLAY").is_some() && !self.has_x11()
    }

    /// One-line summary for logs.
    pub fn describe(&self) -> String {
        format!(
            "DISPLAY={} WAYLAND_DISPLAY={}",
            self.get("DISPLAY").unwrap_or("(unset)"),
            self.get("WAYLAND_DISPLAY").unwrap_or("(unset)")
        )
    }
}

/// Local `:N` needs its socket.
pub fn x11_display_reachable(display: &str, socket_dir: &Path) -> bool {
    let Some(rest) = display.strip_prefix(':') else {
        // Remote display; assume reachable.
        return !display.is_empty();
    };
    let number = rest.split('.').next().unwrap_or_default();
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    socket_dir.join(format!("X{number}")).exists()
}

tokio::task_local! {
    static CURRENT: SessionEnv;
}

/// Run `fut` under `env`.
pub async fn scoped<F: Future>(env: SessionEnv, fut: F) -> F::Output {
    CURRENT.scope(env, fut).await
}

/// Scoped env, else process env.
pub fn current() -> SessionEnv {
    CURRENT
        .try_with(SessionEnv::clone)
        .unwrap_or_else(|_| SessionEnv::capture())
}

/// Whether a scope is installed.
pub fn is_scoped() -> bool {
    CURRENT.try_with(|_| ()).is_ok()
}

/// User override of display session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayTarget {
    /// `tty3` or VT number `3`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tty: Option<String>,
    /// X11 `DISPLAY`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// `WAYLAND_DISPLAY`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wayland_display: Option<String>,
    /// `XAUTHORITY` cookie file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xauthority: Option<String>,
    /// `XDG_RUNTIME_DIR`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xdg_runtime_dir: Option<String>,
}

impl DisplayTarget {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Field-wise: `self` wins over `fallback`.
    pub fn or(&self, fallback: &DisplayTarget) -> DisplayTarget {
        let pick = |a: &Option<String>, b: &Option<String>| a.clone().or_else(|| b.clone());
        DisplayTarget {
            tty: pick(&self.tty, &fallback.tty),
            display: pick(&self.display, &fallback.display),
            wayland_display: pick(&self.wayland_display, &fallback.wayland_display),
            xauthority: pick(&self.xauthority, &fallback.xauthority),
            xdg_runtime_dir: pick(&self.xdg_runtime_dir, &fallback.xdg_runtime_dir),
        }
    }

    fn explicit(&self) -> [(&'static str, Option<&str>); 4] {
        [
            ("DISPLAY", self.display.as_deref()),
            ("WAYLAND_DISPLAY", self.wayland_display.as_deref()),
            ("XAUTHORITY", self.xauthority.as_deref()),
            ("XDG_RUNTIME_DIR", self.xdg_runtime_dir.as_deref()),
        ]
    }
}

/// Per-game over global over base.
pub fn resolve_target(
    base: SessionEnv,
    global: &DisplayTarget,
    game: Option<&DisplayTarget>,
    resolve_tty: impl FnOnce(&str) -> Result<SessionEnv>,
) -> Result<SessionEnv> {
    let target = match game {
        Some(game) => game.or(global),
        None => global.clone(),
    };
    let mut env = match target.tty.as_deref() {
        Some(tty) => {
            let mut from_tty = resolve_tty(tty)?;
            // Keep base vars session lacks.
            for (k, v) in base.vars() {
                if !matches!(k.as_str(), "DISPLAY" | "WAYLAND_DISPLAY" | "XAUTHORITY")
                    && from_tty.get(k).is_none()
                {
                    from_tty.set(k, v);
                }
            }
            from_tty
        }
        None => base,
    };
    for (key, value) in target.explicit() {
        if let Some(value) = value {
            env.set(key, value);
        }
    }
    Ok(env)
}

/// Resolve `play`'s session env from config.
pub fn resolve_for_launch(
    global: &DisplayTarget,
    game: Option<&DisplayTarget>,
) -> Result<SessionEnv> {
    resolve_target(current(), global, game, session_env_for_tty)
}

/// `tty3` or `3` to VT number.
fn tty_vt(tty: &str) -> Option<u32> {
    let t = tty.trim();
    t.strip_prefix("tty").unwrap_or(t).parse().ok()
}

/// Session id for `tty` from loginctl.
pub fn session_id_for_tty(show_session: &str, tty: &str, uid: u32) -> Option<String> {
    let want_vt = tty_vt(tty);
    let want_tty = tty.trim();
    for block in show_session.split("\n\n") {
        let props: HashMap<&str, &str> = block.lines().filter_map(|l| l.split_once('=')).collect();
        if props.get("User").and_then(|u| u.parse::<u32>().ok()) != Some(uid) {
            continue;
        }
        let tty_match = props.get("TTY") == Some(&want_tty)
            || want_vt.is_some_and(|vt| props.get("VTNr").and_then(|v| v.parse().ok()) == Some(vt));
        if tty_match && let Some(id) = props.get("Id") {
            return Some((*id).to_string());
        }
    }
    None
}

/// Per-var majority over session processes.
pub fn vote_session_env(session_id: &str, environs: &[HashMap<String, String>]) -> SessionEnv {
    let members: Vec<&HashMap<String, String>> = environs
        .iter()
        .filter(|e| e.get("XDG_SESSION_ID").map(String::as_str) == Some(session_id))
        .filter(|e| e.contains_key("DISPLAY") || e.contains_key("WAYLAND_DISPLAY"))
        .filter(|e| {
            !e.keys().any(|k| k.starts_with("PRESSURE_VESSEL"))
                && !e
                    .get("XAUTHORITY")
                    .is_some_and(|x| x.starts_with("/run/pressure-vessel"))
        })
        .collect();
    let mut env = SessionEnv::default();
    for key in SESSION_VARS {
        let mut counts: BTreeMap<Option<&str>, usize> = BTreeMap::new();
        for e in &members {
            *counts.entry(e.get(key).map(String::as_str)).or_default() += 1;
        }
        // Ties keep first; None first.
        let winner = counts
            .iter()
            .fold(None::<(Option<&str>, usize)>, |best, (v, n)| match best {
                Some((_, bn)) if bn >= *n => best,
                _ => Some((*v, *n)),
            });
        if let Some((Some(value), _)) = winner {
            env.set(key, value);
        }
    }
    env
}

/// Env of the session on `tty`.
#[cfg(target_os = "linux")]
pub fn session_env_for_tty(tty: &str) -> Result<SessionEnv> {
    let uid = unsafe { libc::getuid() };
    let list = std::process::Command::new("loginctl")
        .args(["list-sessions", "--no-legend"])
        .output()
        .context("could not run loginctl to resolve the display tty")?;
    let ids: Vec<String> = String::from_utf8_lossy(&list.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect();
    if ids.is_empty() {
        bail!("loginctl lists no sessions; can't resolve display tty {tty:?}");
    }
    let show = std::process::Command::new("loginctl")
        .arg("show-session")
        .args(&ids)
        .args(["-p", "Id", "-p", "User", "-p", "TTY", "-p", "VTNr"])
        .output()
        .context("could not run loginctl show-session")?;
    let show = String::from_utf8_lossy(&show.stdout);
    let Some(id) = session_id_for_tty(&show, tty, uid) else {
        bail!("no session of yours runs on {tty:?} (see `loginctl list-sessions`)");
    };
    let environs = own_process_environs(uid);
    let env = vote_session_env(&id, &environs);
    if env.get("DISPLAY").is_none() && env.get("WAYLAND_DISPLAY").is_none() {
        bail!(
            "session {id} on {tty:?} has no process with a display env yet; \
             is a graphical session running there?"
        );
    }
    tracing::info!(tty, session = %id, "resolved display session: {}", env.describe());
    Ok(env)
}

#[cfg(not(target_os = "linux"))]
pub fn session_env_for_tty(tty: &str) -> Result<SessionEnv> {
    bail!("display tty {tty:?}: resolving sessions by tty is only supported on Linux")
}

/// Environs of this user's readable processes.
#[cfg(target_os = "linux")]
fn own_process_environs(uid: u32) -> Vec<HashMap<String, String>> {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
        })
        .filter(|e| e.metadata().is_ok_and(|m| m.uid() == uid))
        .filter_map(|e| std::fs::read(e.path().join("environ")).ok())
        .map(|raw| parse_environ(&raw))
        .collect()
}

/// Parse a NUL-separated environ blob.
pub fn parse_environ(raw: &[u8]) -> HashMap<String, String> {
    raw.split(|b| *b == 0)
        .filter_map(|kv| {
            let kv = String::from_utf8_lossy(kv);
            kv.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests;
