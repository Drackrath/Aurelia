use std::collections::HashMap;
use crate::launch::pipeline::PipelineContext;
use crate::launch::validators::LaunchValidator;

pub struct OverrideConflictValidator;

impl LaunchValidator for OverrideConflictValidator {
    fn name(&self) -> &str { "OverrideConflict" }

    fn validate(&self, ctx: &mut PipelineContext) {
        let mut warnings = Vec::new();

        if let Some(user_config) = &ctx.user_config {
            let dxvk_enabled = user_config.graphics_layers.dxvk_enabled;
            let vkd3d_proton_enabled = user_config.graphics_layers.vkd3d_proton_enabled;

            // 1. Check for D3D/DXGI override conflicts with DXVK
            if dxvk_enabled {
                let conflicts = ["d3d9", "d3d10core", "d3d11", "dxgi"];
                if let Some(val) = user_config.env_variables.get("WINEDLLOVERRIDES") {
                    for part in val.split(';') {
                        if let Some((dll, mode)) = part.split_once('=') {
                            let dll_trimmed = dll.trim().to_lowercase();
                            if conflicts.contains(&dll_trimmed.as_str()) && mode.contains('b') {
                                warnings.push((
                                    "OVERRIDE_CONFLICT_DXVK",
                                    format!("Manual override '{dll}={mode}' may conflict with enabled DXVK layer.")
                                ));
                            }
                        }
                    }
                }
            }

            // 2. Check for D3D12 override conflicts with VKD3D-Proton
            if vkd3d_proton_enabled {
                let conflicts = ["d3d12", "d3d12core"];
                if let Some(val) = user_config.env_variables.get("WINEDLLOVERRIDES") {
                    for part in val.split(';') {
                        if let Some((dll, mode)) = part.split_once('=') {
                            let dll_trimmed = dll.trim().to_lowercase();
                            if conflicts.contains(&dll_trimmed.as_str()) && mode.contains('b') {
                                warnings.push((
                                    "OVERRIDE_CONFLICT_VKD3D",
                                    format!("Manual override '{dll}={mode}' may conflict with enabled VKD3D-Proton layer.")
                                ));
                            }
                        }
                    }
                }
            }

            // 3. Check for contradictory values in WINEDLLOVERRIDES string
            if let Some(val) = user_config.env_variables.get("WINEDLLOVERRIDES") {
                let mut seen_dlls: HashMap<String, String> = HashMap::new();
                for part in val.split(';') {
                    if let Some((dll, mode)) = part.split_once('=') {
                        let dll_trimmed = dll.trim().to_lowercase();
                        let mode_trimmed = mode.trim().to_lowercase();
                        if let Some(prev_mode) = seen_dlls.get(&dll_trimmed) {
                            if prev_mode != &mode_trimmed {
                                warnings.push((
                                    "OVERRIDE_CONTRADICTION",
                                    format!("Contradictory overrides for '{dll}': '{prev_mode}' and '{mode_trimmed}'.")
                                ));
                            }
                        }
                        seen_dlls.insert(dll_trimmed, mode_trimmed);
                    }
                }
            }
        }

        for (code, msg) in warnings {
            ctx.add_warning(code, msg);
        }
    }
}
