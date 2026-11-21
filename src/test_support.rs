//! Shared helpers for unit tests.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

/// Serializes tests that change process env.
pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Env overrides, restored on drop.
pub(crate) struct EnvGuard {
    saved: Vec<(String, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// Lock, then set or unset each variable.
    pub(crate) fn set(vars: &[(&str, Option<&OsStr>)]) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut saved = Vec::new();
        for (key, value) in vars {
            saved.push((key.to_string(), std::env::var_os(key)));
            // SAFETY: writers hold ENV_LOCK.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
        Self { saved, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..).rev() {
            // SAFETY: still holding ENV_LOCK.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(&key, v),
                    None => std::env::remove_var(&key),
                }
            }
        }
    }
}

/// Write `steamapps/libraryfolders.vdf` listing `libs`.
pub(crate) fn write_libraryfolders(root: &Path, libs: &[&Path]) {
    let mut body = String::from("\"libraryfolders\"\n{\n");
    for (i, lib) in libs.iter().enumerate() {
        body.push_str(&format!(
            "\t\"{i}\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n",
            lib.display()
        ));
    }
    body.push_str("}\n");
    let steamapps = root.join("steamapps");
    std::fs::create_dir_all(&steamapps).unwrap();
    std::fs::write(steamapps.join("libraryfolders.vdf"), body).unwrap();
}

/// Create an executable shell stub.
pub(crate) fn make_exec(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}
