//! Opt-in native Wayland launches.

use std::collections::HashMap;
use std::path::Path;

/// Wine's Wayland switch in Proton.
pub const PROTON_FLAG: &str = "PROTON_ENABLE_WAYLAND";

/// Outcome of planning a Wayland launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaylandPlan {
    /// Set these vars.
    Enable(Vec<(&'static str, &'static str)>),
    /// Runner lacks the driver.
    Unsupported(String),
}

/// Plan for a resolved runner, or native.
pub fn plan(runner_root: Option<&Path>) -> WaylandPlan {
    match runner_root {
        Some(root) if crate::compat::proton::supports_wine_wayland(root) => {
            WaylandPlan::Enable(vec![(PROTON_FLAG, "1")])
        }
        Some(root) => WaylandPlan::Unsupported(format!(
            "{} ships no winewayland.drv; use GE-Proton 10+ or Proton-CachyOS",
            root.file_name()
                .unwrap_or(root.as_os_str())
                .to_string_lossy()
        )),
        // Runner unknown yet; both harmless.
        None => WaylandPlan::Enable(vec![(PROTON_FLAG, "1"), ("SDL_VIDEODRIVER", "wayland")]),
    }
}

/// Insert vars the user hasn't set.
pub fn apply(env: &mut HashMap<String, String>, vars: &[(&str, &str)]) {
    for (k, v) in vars {
        env.entry((*k).to_string())
            .or_insert_with(|| (*v).to_string());
    }
}

#[cfg(test)]
mod tests;
