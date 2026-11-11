//! Proton anti-cheat runtimes (EAC, BattlEye).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Anti-cheat with a Proton runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AntiCheat {
    EasyAntiCheat,
    BattlEye,
}

impl AntiCheat {
    pub const ALL: [AntiCheat; 2] = [AntiCheat::EasyAntiCheat, AntiCheat::BattlEye];

    /// Steam app id of the runtime.
    pub fn runtime_app_id(self) -> u32 {
        match self {
            AntiCheat::EasyAntiCheat => 1826330,
            AntiCheat::BattlEye => 1161040,
        }
    }

    pub fn runtime_name(self) -> &'static str {
        match self {
            AntiCheat::EasyAntiCheat => "Proton EasyAntiCheat Runtime",
            AntiCheat::BattlEye => "Proton BattlEye Runtime",
        }
    }

    /// Name used by `proton anticheat`.
    pub fn cli_name(self) -> &'static str {
        match self {
            AntiCheat::EasyAntiCheat => "eac",
            AntiCheat::BattlEye => "battleye",
        }
    }

    /// Variable Proton reads the path from.
    pub fn env_var(self) -> &'static str {
        match self {
            AntiCheat::EasyAntiCheat => "PROTON_EAC_RUNTIME",
            AntiCheat::BattlEye => "PROTON_BATTLEYE_RUNTIME",
        }
    }

    fn marker(self, name: &str, is_dir: bool) -> bool {
        let name = name.to_ascii_lowercase();
        match self {
            AntiCheat::EasyAntiCheat => {
                (is_dir && name == "easyanticheat")
                    || (!is_dir && name == "start_protected_game.exe")
            }
            AntiCheat::BattlEye => {
                (is_dir && name == "battleye") || (!is_dir && name == "beclient_x64.dll")
            }
        }
    }
}

/// How deep markers are searched.
const MAX_DEPTH: usize = 4;
/// Entries scanned before giving up.
const MAX_ENTRIES: usize = 20_000;

/// Anti-cheats shipped with a game install.
pub fn detect(install_dir: &Path) -> Vec<AntiCheat> {
    let mut found = Vec::new();
    let mut budget = MAX_ENTRIES;
    walk(install_dir, 0, &mut budget, &mut found);
    AntiCheat::ALL
        .into_iter()
        .filter(|kind| found.contains(kind))
        .collect()
}

fn walk(dir: &Path, depth: usize, budget: &mut usize, found: &mut Vec<AntiCheat>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        if *budget == 0 || found.len() == AntiCheat::ALL.len() {
            return;
        }
        *budget -= 1;
        // no symlinks: avoids loops
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let is_dir = file_type.is_dir();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        for kind in AntiCheat::ALL {
            if !found.contains(&kind) && kind.marker(&name, is_dir) {
                found.push(kind);
            }
        }
        if is_dir && depth + 1 < MAX_DEPTH {
            subdirs.push(entry.path());
        }
    }
    for sub in subdirs {
        walk(&sub, depth + 1, budget, found);
    }
}

/// Env pairs for installed runtimes.
pub fn runtime_env(
    kinds: &[AntiCheat],
    installed: &HashMap<u32, PathBuf>,
) -> Vec<(String, String)> {
    kinds
        .iter()
        .filter_map(|kind| {
            let path = installed.get(&kind.runtime_app_id())?;
            Some((
                kind.env_var().to_string(),
                path.to_string_lossy().into_owned(),
            ))
        })
        .collect()
}

/// Installed runtimes in Linux Steam libraries.
pub async fn installed_runtimes() -> HashMap<u32, PathBuf> {
    let Ok(apps) = crate::library::scan_installed_app_info().await else {
        return HashMap::new();
    };
    AntiCheat::ALL
        .into_iter()
        .filter_map(|kind| {
            let id = kind.runtime_app_id();
            let info = apps.get(&id)?;
            (!info.from_windows_steam && info.install_path.is_dir())
                .then(|| (id, info.install_path.clone()))
        })
        .collect()
}
