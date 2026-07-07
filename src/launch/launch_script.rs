//! Per-game launch scripts.
//!
//! A launch script is a user-supplied shell script (`<app_id>.sh` on unix,
//! `<app_id>.bat` on Windows) that Aurelia runs as a **wrapper** around the
//! fully-resolved launch command. When a script is active for an app the pipeline
//! rewrites the resolved [`CommandSpec`](crate::infra::runners::CommandSpec) so the
//! script becomes the program and the previously-resolved program + args are passed
//! as its arguments (`"$@"`). A script that is just `exec "$@"` is therefore a
//! transparent passthrough, while a custom one can prepend `gamemoderun` / `mangohud`
//! / `gamescope` or launch its own way. This works uniformly for native, Proton
//! (WineTkg), luxtorpeda and umu launches because it operates on the final spec.
//!
//! Aurelia also exports `AURELIA_*` env vars (see [`template`]) alongside the full
//! launch environment (WINEPREFIX etc.) so scripts can introspect the launch.

use std::path::{Path, PathBuf};

use crate::core::config::LauncherConfig;

/// Directory that holds per-game launch scripts.
///
/// `AURELIA_SCRIPT_DIR` overrides it; otherwise it is `<config_dir>/scripts`. If the
/// config dir cannot be resolved we fall back to a relative `scripts` directory so
/// callers stay infallible (this only happens when even `$HOME` is unset, in which
/// case nothing else in Aurelia works either).
pub fn script_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("AURELIA_SCRIPT_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    match crate::core::config::config_dir() {
        Ok(dir) => dir.join("scripts"),
        Err(_) => PathBuf::from("scripts"),
    }
}

/// Platform-specific script filename for an app id (`<app_id>.sh` on unix,
/// `<app_id>.bat` on Windows).
pub fn script_filename(app_id: u32) -> String {
    if cfg!(windows) {
        format!("{app_id}.bat")
    } else {
        format!("{app_id}.sh")
    }
}

/// Auto-detected on-disk path for an app's launch script: `script_dir()/<file>`.
pub fn dir_script_path(app_id: u32) -> PathBuf {
    script_dir().join(script_filename(app_id))
}

/// Resolve the launch script to use for `app_id`, honoring precedence.
///
/// * `disabled` (e.g. `play --no-script`) short-circuits to `None`.
/// * Otherwise, precedence is: (1) one-off `override_path` (`play --script`),
///   (2) the per-game `config.game_configs[app_id].launch_script`, (3) the
///   auto-detected [`dir_script_path`] when it exists on disk.
///
/// The `override_path` and config paths are returned **unconditionally** (even when
/// missing) so the apply stage can surface a clear `Validation` error for a bad
/// explicit path rather than silently falling back to a lower-precedence script. The
/// auto-detected path is only returned when it actually exists.
pub fn resolve(
    app_id: u32,
    config: Option<&LauncherConfig>,
    override_path: Option<&Path>,
    disabled: bool,
) -> Option<PathBuf> {
    if disabled {
        return None;
    }

    // (1) One-off override wins.
    if let Some(p) = override_path {
        return Some(p.to_path_buf());
    }

    // (2) Per-game config-pinned script.
    if let Some(script) = config
        .and_then(|c| c.game_configs.get(&app_id))
        .and_then(|g| g.launch_script.as_ref())
        .filter(|s| !s.is_empty())
    {
        return Some(PathBuf::from(script));
    }

    // (3) Auto-detected `<dir>/<app_id>.sh|bat`, only when present on disk.
    let dir_path = dir_script_path(app_id);
    if dir_path.exists() {
        return Some(dir_path);
    }

    None
}

/// A scaffold launch script for `app_id` / `name`, documenting the wrapper
/// semantics and the exported `AURELIA_*` env vars, with commented examples and a
/// final passthrough (`exec "$@"` on unix; `%AURELIA_LAUNCH_PROGRAM% %*` on Windows).
pub fn template(app_id: u32, name: &str) -> String {
    let name = if name.trim().is_empty() { "Unknown Game" } else { name };

    #[cfg(not(windows))]
    {
        format!(
            "#!/usr/bin/env bash\n\
             # Aurelia per-game launch script for app {app_id} ({name}).\n\
             #\n\
             # Aurelia runs this script as a WRAPPER around the fully-resolved launch\n\
             # command: the resolved program and its arguments are passed to this script\n\
             # as \"$@\", and the entire launch environment (WINEPREFIX, WINEDLLOVERRIDES,\n\
             # STEAM_COMPAT_*, DXVK_HUD, ...) is already exported. A script that is just\n\
             # `exec \"$@\"` is therefore a transparent passthrough.\n\
             #\n\
             # Aurelia additionally exports:\n\
             #   AURELIA_APP_ID        - the Steam app id ({app_id})\n\
             #   AURELIA_APP_NAME      - the game's display name\n\
             #   AURELIA_GAME_DIR      - the game's install directory (if known)\n\
             #   AURELIA_LAUNCH_PROGRAM - the resolved program that would have run\n\
             #   AURELIA_LAUNCH_ARGS   - its arguments, space-joined\n\
             #\n\
             # Examples (uncomment one, or write your own):\n\
             #   exec gamemoderun mangohud \"$@\"\n\
             #   exec gamescope -W 2560 -H 1440 -- \"$@\"\n\
             #\n\
             # Default: run the resolved command unchanged.\n\
             exec \"$@\"\n"
        )
    }

    #[cfg(windows)]
    {
        format!(
            "@echo off\r\n\
             rem Aurelia per-game launch script for app {app_id} ({name}).\r\n\
             rem\r\n\
             rem Aurelia runs this script as a WRAPPER around the fully-resolved launch\r\n\
             rem command: the resolved program and its arguments are passed to this script\r\n\
             rem as %*, and the launch environment is already exported. A script that just\r\n\
             rem runs %AURELIA_LAUNCH_PROGRAM% %* is a transparent passthrough.\r\n\
             rem\r\n\
             rem Aurelia additionally exports:\r\n\
             rem   %%AURELIA_APP_ID%%         - the Steam app id ({app_id})\r\n\
             rem   %%AURELIA_APP_NAME%%       - the game's display name\r\n\
             rem   %%AURELIA_GAME_DIR%%       - the game's install directory (if known)\r\n\
             rem   %%AURELIA_LAUNCH_PROGRAM%% - the resolved program that would have run\r\n\
             rem   %%AURELIA_LAUNCH_ARGS%%    - its arguments, space-joined\r\n\
             rem\r\n\
             rem Examples (uncomment one, or write your own):\r\n\
             rem   mangohud %*\r\n\
             rem\r\n\
             rem Default: run the resolved command unchanged.\r\n\
             %AURELIA_LAUNCH_PROGRAM% %*\r\n"
        )
    }
}

/// App ids that have a launch script: any `<id>.sh`/`<id>.bat` in [`script_dir`]
/// plus any per-game config-pinned script. Sorted and de-duplicated.
pub fn list_script_app_ids(config: Option<&LauncherConfig>) -> Vec<u32> {
    let mut ids = std::collections::BTreeSet::new();

    if let Ok(entries) = std::fs::read_dir(script_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_script = matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("sh") | Some("bat")
            );
            if is_script {
                if let Some(id) = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse::<u32>().ok())
                {
                    ids.insert(id);
                }
            }
        }
    }

    if let Some(cfg) = config {
        for (id, gc) in &cfg.game_configs {
            if gc.launch_script.as_ref().filter(|s| !s.is_empty()).is_some() {
                ids.insert(*id);
            }
        }
    }

    ids.into_iter().collect()
}

/// Serializes tests that mutate the process-global `AURELIA_SCRIPT_DIR` env var so
/// they don't race each other (across this module and the apply-stage tests).
#[cfg(test)]
pub(crate) static SCRIPT_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Delete the auto-detected dir-based script for `app_id`. Returns `Ok(true)` if a
/// file was removed, `Ok(false)` if none existed.
pub fn remove_dir_script(app_id: u32) -> std::io::Result<bool> {
    let path = dir_script_path(app_id);
    if path.exists() {
        std::fs::remove_file(&path)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{GameConfig, LauncherConfig};
    use std::collections::HashMap;
    use tempfile::tempdir;

    fn config_with_script(app_id: u32, script: Option<&str>) -> LauncherConfig {
        let mut cfg = LauncherConfig::default();
        let mut gc = GameConfig::default();
        gc.launch_script = script.map(|s| s.to_string());
        let mut map = HashMap::new();
        map.insert(app_id, gc);
        cfg.game_configs = map;
        cfg
    }

    #[test]
    fn disabled_returns_none_even_with_override() {
        let tmp = tempdir().unwrap();
        let script = tmp.path().join("custom.sh");
        std::fs::write(&script, "exec \"$@\"\n").unwrap();
        let cfg = config_with_script(42, Some(script.to_string_lossy().as_ref()));
        assert!(resolve(42, Some(&cfg), Some(&script), true).is_none());
    }

    #[test]
    fn override_beats_config() {
        let over = PathBuf::from("/some/override.sh");
        let cfg = config_with_script(42, Some("/config/pinned.sh"));
        let got = resolve(42, Some(&cfg), Some(&over), false);
        assert_eq!(got, Some(over));
    }

    #[test]
    fn config_beats_dir() {
        // No override; a config-pinned path is returned (even if missing) ahead of
        // any auto-detected dir script.
        let cfg = config_with_script(42, Some("/config/pinned.sh"));
        let got = resolve(42, Some(&cfg), None, false);
        assert_eq!(got, Some(PathBuf::from("/config/pinned.sh")));
    }

    #[test]
    fn dir_script_used_when_present() {
        let _guard = SCRIPT_DIR_ENV_LOCK.lock().unwrap();
        let tmp = tempdir().unwrap();
        // Point AURELIA_SCRIPT_DIR at a temp dir with an on-disk script.
        unsafe { std::env::set_var("AURELIA_SCRIPT_DIR", tmp.path()) };
        let path = dir_script_path(4242);
        std::fs::write(&path, "exec \"$@\"\n").unwrap();

        let got = resolve(4242, None, None, false);
        assert_eq!(got, Some(path));

        // No script for a different id.
        assert!(resolve(9999, None, None, false).is_none());
        unsafe { std::env::remove_var("AURELIA_SCRIPT_DIR") };
    }

    #[test]
    fn empty_config_script_is_ignored() {
        let _guard = SCRIPT_DIR_ENV_LOCK.lock().unwrap();
        let cfg = config_with_script(42, Some(""));
        // Empty string is treated as unset; with no override and no dir script -> None.
        let tmp = tempdir().unwrap();
        unsafe { std::env::set_var("AURELIA_SCRIPT_DIR", tmp.path()) };
        let got = resolve(42, Some(&cfg), None, false);
        assert!(got.is_none());
        unsafe { std::env::remove_var("AURELIA_SCRIPT_DIR") };
    }

    #[test]
    #[cfg(not(windows))]
    fn template_unix_has_shebang_and_passthrough() {
        let t = template(1234, "Test Game");
        assert!(t.starts_with("#!/usr/bin/env bash"), "missing shebang: {t}");
        assert!(t.contains("\"$@\""), "missing \"$@\": {t}");
        assert!(t.contains("1234"));
        assert!(t.contains("AURELIA_APP_ID"));
    }

    #[test]
    #[cfg(windows)]
    fn template_windows_has_passthrough() {
        let t = template(1234, "Test Game");
        assert!(t.contains("%*"), "missing %*: {t}");
        assert!(t.contains("%AURELIA_LAUNCH_PROGRAM%"), "missing passthrough: {t}");
        assert!(t.contains("1234"));
        assert!(t.contains("AURELIA_APP_ID"));
    }

    #[test]
    fn filename_is_platform_specific() {
        let f = script_filename(77);
        if cfg!(windows) {
            assert_eq!(f, "77.bat");
        } else {
            assert_eq!(f, "77.sh");
        }
    }
}
