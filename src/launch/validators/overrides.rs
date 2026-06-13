use std::collections::HashMap;
use crate::launch::pipeline::PipelineContext;
use crate::launch::validators::LaunchValidator;

pub struct OverrideConflictValidator;

impl LaunchValidator for OverrideConflictValidator {
    fn name(&self) -> &str { "OverrideConflict" }

    fn validate(&self, ctx: &mut PipelineContext) {
        let mut warnings = Vec::new();

        if let Some(user_config) = &ctx.user_config {
            let overrides = user_config.env_variables.get("WINEDLLOVERRIDES");
            // Each override is a "dll=mode" pair in a ';'-separated string.
            let parse = || {
                overrides
                    .into_iter()
                    .flat_map(|val| val.split(';'))
                    .filter_map(|part| part.split_once('='))
            };

            // Check D3D/DXGI overrides against enabled graphics layers.
            let mut check_conflicts = |enabled: bool, conflicts: &[&str], code, layer| {
                if !enabled {
                    return;
                }
                for (dll, mode) in parse() {
                    if conflicts.contains(&dll.trim().to_lowercase().as_str()) && mode.contains('b') {
                        warnings.push((
                            code,
                            format!("Manual override '{dll}={mode}' may conflict with enabled {layer} layer."),
                        ));
                    }
                }
            };
            check_conflicts(
                user_config.graphics_layers.dxvk_enabled,
                &["d3d9", "d3d10core", "d3d11", "dxgi"],
                "OVERRIDE_CONFLICT_DXVK",
                "DXVK",
            );
            check_conflicts(
                user_config.graphics_layers.vkd3d_proton_enabled,
                &["d3d12", "d3d12core"],
                "OVERRIDE_CONFLICT_VKD3D",
                "VKD3D-Proton",
            );

            // Check for contradictory values in WINEDLLOVERRIDES string.
            let mut seen_dlls: HashMap<String, String> = HashMap::new();
            for (dll, mode) in parse() {
                let dll_trimmed = dll.trim().to_lowercase();
                let mode_trimmed = mode.trim().to_lowercase();
                if let Some(prev_mode) = seen_dlls.get(&dll_trimmed) {
                    if prev_mode != &mode_trimmed {
                        warnings.push((
                            "OVERRIDE_CONTRADICTION",
                            format!("Contradictory overrides for '{dll}': '{prev_mode}' and '{mode_trimmed}'."),
                        ));
                    }
                }
                seen_dlls.insert(dll_trimmed, mode_trimmed);
            }
        }

        for (code, msg) in warnings {
            ctx.add_warning(code, msg);
        }
    }
}
