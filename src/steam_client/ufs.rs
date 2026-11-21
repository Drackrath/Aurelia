//! Steam Cloud (UFS) config from PICS.

use super::*;

/// Steam Auto-Cloud save-file rules from an app's `ufs/savefiles` PICS section.
/// Each entry names a root token, sub-path, filename glob, and recursion flag.
pub(crate) fn ufs_save_specs_from_section(section: &steam_vdf_parser::Value) -> Vec<UfsSaveSpec> {
    let mut specs = Vec::new();
    if let Some(savefiles) = section.get_obj(&["ufs", "savefiles"]) {
        for (_, entry) in savefiles.iter() {
            let get = |k: &str| entry.get(k).and_then(|v| v.as_str());
            let root = get("root").unwrap_or("").to_string();
            if root.is_empty() {
                continue;
            }
            specs.push(UfsSaveSpec {
                root,
                path: get("path").unwrap_or("").to_string(),
                pattern: get("pattern").unwrap_or("*").to_string(),
                recursive: entry.get("recursive").is_some_and(vdf_flag),
            });
        }
    }
    specs
}

/// Flag: string "1" or int 1.
pub(crate) fn vdf_flag(value: &steam_vdf_parser::Value) -> bool {
    match value.as_str() {
        Some(s) => s.trim() == "1",
        None => value.as_i32().is_some_and(|n| n != 0) || value.as_u64().is_some_and(|n| n != 0),
    }
}

/// `ufs/rootoverrides` rules from PICS.
pub(crate) fn ufs_root_overrides_from_section(
    section: &steam_vdf_parser::Value,
) -> Vec<UfsRootOverride> {
    let Some(overrides) = section.get_obj(&["ufs", "rootoverrides"]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (_, entry) in overrides.iter() {
        let get = |k: &str| {
            entry
                .get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let root = get("root");
        if root.is_empty() {
            continue;
        }
        let mut path_transforms = Vec::new();
        if let Some(transforms) = entry.get("pathtransforms").and_then(|v| v.as_obj()) {
            for (_, t) in transforms.iter() {
                let find = t.get("find").and_then(|v| v.as_str()).unwrap_or("");
                let replace = t.get("replace").and_then(|v| v.as_str()).unwrap_or("");
                if !find.is_empty() {
                    path_transforms.push((find.to_string(), replace.to_string()));
                }
            }
        }
        out.push(UfsRootOverride {
            root,
            os: get("os"),
            os_compare: get("oscompare"),
            use_instead: get("useinstead"),
            add_path: get("addpath"),
            path_transforms,
        });
    }
    out
}

/// Full UFS config from PICS.
pub(crate) fn ufs_config_from_section(section: &steam_vdf_parser::Value) -> UfsConfig {
    UfsConfig {
        savefiles: ufs_save_specs_from_section(section),
        root_overrides: ufs_root_overrides_from_section(section),
    }
}
