//! WINEDLLOVERRIDES parsing and merging.

/// Lowercase DLL name without `.dll`.
pub fn normalize_dll_name(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    lower.strip_suffix(".dll").unwrap_or(&lower).to_string()
}

/// Splits into `(names, mode, raw)` groups.
fn parse_dll_override_groups(overrides: &str) -> Vec<(Vec<String>, Option<String>, String)> {
    overrides
        .split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|entry| match entry.split_once('=') {
            Some((names, mode)) => (
                names
                    .split(',')
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .collect(),
                Some(mode.trim().to_string()),
                entry.to_string(),
            ),
            None => (Vec::new(), None, entry.to_string()),
        })
        .collect()
}

/// Current mode for `dll`, if any.
pub fn dll_override_mode(overrides: &str, dll: &str) -> Option<String> {
    let key = normalize_dll_name(dll);
    parse_dll_override_groups(overrides)
        .into_iter()
        .rev()
        .find_map(|(names, mode, _)| {
            names
                .iter()
                .any(|n| normalize_dll_name(n) == key)
                .then_some(mode)
                .flatten()
        })
}

/// Normalised DLL names in overrides.
pub fn dll_override_names(overrides: &str) -> std::collections::HashSet<String> {
    parse_dll_override_groups(overrides)
        .into_iter()
        .flat_map(|(names, _, _)| names)
        .map(|n| normalize_dll_name(&n))
        .collect()
}

/// Merges by DLL name; `entries` win.
pub fn merge_dll_overrides(existing: &str, entries: &[(String, String)]) -> String {
    // Last entry wins per DLL.
    let mut new_entries: Vec<(String, String, String)> = Vec::new();
    for (name, mode) in entries {
        let key = normalize_dll_name(name);
        if key.is_empty() {
            continue;
        }
        new_entries.retain(|(k, _, _)| *k != key);
        new_entries.push((key, name.trim().to_string(), mode.trim().to_string()));
    }
    let replaced: std::collections::HashSet<&str> =
        new_entries.iter().map(|(k, _, _)| k.as_str()).collect();

    let mut out: Vec<String> = Vec::new();
    for (names, mode, raw) in parse_dll_override_groups(existing) {
        let Some(mode) = mode else {
            out.push(raw);
            continue;
        };
        let kept: Vec<String> = names
            .into_iter()
            .filter(|n| !replaced.contains(normalize_dll_name(n).as_str()))
            .collect();
        if !kept.is_empty() {
            out.push(format!("{}={}", kept.join(","), mode));
        }
    }
    out.extend(
        new_entries
            .into_iter()
            .map(|(_, name, mode)| format!("{name}={mode}")),
    );
    out.join(";")
}

/// Adds only DLLs not yet present.
pub fn fill_dll_overrides(existing: &str, entries: &[(String, String)]) -> String {
    let present = dll_override_names(existing);
    let missing: Vec<(String, String)> = entries
        .iter()
        .filter(|(name, _)| !present.contains(&normalize_dll_name(name)))
        .cloned()
        .collect();
    merge_dll_overrides(existing, &missing)
}

/// Parses user overrides into entries.
pub fn dll_override_entries(overrides: &str) -> Vec<(String, String)> {
    parse_dll_override_groups(overrides)
        .into_iter()
        .filter_map(|(names, mode, _)| mode.map(|m| (names, m)))
        .flat_map(|(names, mode)| names.into_iter().map(move |n| (n, mode.clone())))
        .collect()
}

/// Inputs for `build_dll_overrides`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DllOverrideInputs<'a> {
    pub dxvk_active: bool,
    pub vkd3d_proton_active: bool,
    pub vkd3d_active: bool,
    pub no_overlay: bool,
    pub force_builtin_d3d: bool,
    pub game_dir: Option<&'a std::path::Path>,
    pub strict_dxvk: bool,
    pub steam_enabled: bool,
}

/// Returns the WINEDLLOVERRIDES string needed to activate installed layers.
pub fn build_dll_overrides(inputs: DllOverrideInputs<'_>) -> String {
    let DllOverrideInputs {
        dxvk_active,
        vkd3d_proton_active,
        vkd3d_active,
        no_overlay,
        force_builtin_d3d,
        game_dir,
        strict_dxvk,
        steam_enabled,
    } = inputs;
    // In standalone mode Aurelia neutralises Steam's client DLLs and disables
    // `lsteamclient` so a game runs without a Steam client. When Steam is enabled
    // those must be left to Proton's defaults (`lsteamclient` loads builtin and
    // bridges to the running client), otherwise Steamworks init fails and
    // Family-Shared / online games can't start.
    let mut overrides: Vec<String> = if steam_enabled {
        Vec::new()
    } else {
        vec![
            "vstdlib_s=n".into(),
            "tier0_s=n".into(),
            "steamclient=n".into(),
            "steamclient64=n".into(),
            "steam_api=n".into(),
            "steam_api64=n".into(),
            "lsteamclient=".into(),
        ]
    };

    if no_overlay {
        overrides.push("GameOverlayRenderer=n".into());
        overrides.push("GameOverlayRenderer64=n".into());
    }

    if force_builtin_d3d {
        // Explicitly force Wine's own builtins for all D3D DLLs.
        // This overrides any native DLL sitting in the prefix's system32
        // from a previous DXVK/VKD3D install.
        for dll in &[
            "d3d8",
            "d3d9",
            "d3d10core",
            "d3d11",
            "dxgi",
            "d3d12",
            "d3d12core",
        ] {
            overrides.push(format!("{dll}=b"));
        }
        return overrides.join(";");
    }

    if dxvk_active {
        // If the game ships its own d3d DLLs, don't fight them — just
        // ensure native wins without specifying which native.
        // Wine searches exe-dir before system32, so "n,b" is fine UNLESS
        // a foreign dll landed in system32. We skip the override entirely
        // for DLLs the game already provides locally.
        let game_has = |dll: &str| -> bool { game_dir.is_some_and(|d| d.join(dll).exists()) };

        for dll in &[
            "d3d8.dll",
            "d3d9.dll",
            "d3d10core.dll",
            "d3d11.dll",
            "dxgi.dll",
        ] {
            let stem = dll.trim_end_matches(".dll");
            let mode = if strict_dxvk { "n" } else { "n,b" };

            if strict_dxvk || !game_has(dll) {
                overrides.push(format!("{stem}={mode}"));
            }
            // If the game ships it locally and we are not in strict mode,
            // leave Wine's default search order alone — exe-dir native wins automatically.
        }
    }

    if vkd3d_proton_active || vkd3d_active {
        overrides.push("d3d12=n,b".into());
        overrides.push("d3d12core=n,b".into());
        if vkd3d_active {
            overrides.push("libvkd3d-1=n,b".into());
            overrides.push("libvkd3d-shader-1=n,b".into());
        }
    }

    overrides.join(";")
}

#[cfg(test)]
mod tests;
