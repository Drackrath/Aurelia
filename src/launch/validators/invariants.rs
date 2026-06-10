use crate::launch::pipeline::PipelineContext;
use crate::launch::validators::LaunchValidator;

pub struct LaunchInvariantValidator;

impl LaunchValidator for LaunchInvariantValidator {
    fn name(&self) -> &str { "LaunchInvariant" }

    fn validate(&self, ctx: &mut PipelineContext) {
        let mut warnings = Vec::new();

        // Invariant A: If effective GPU is unset/default, there must be no forced GPU-selection env vars.
        if ctx.graphics_stack.effective_gpu.is_none() {
            if let Some(spec) = &ctx.command_spec {
                for key in &[
                    "DRI_PRIME",
                    "__NV_PRIME_RENDER_OFFLOAD",
                    "__NV_PRIME_RENDER_OFFLOAD_PROVIDER",
                    "__GLX_VENDOR_LIBRARY_NAME",
                    "__VK_LAYER_NV_optimus",
                ] {
                    if spec.env.contains_key(*key) {
                        warnings.push((
                            "INVARIANT_A_VIOLATION",
                            format!("Effective GPU is unset but found GPU-forcing env var: {}", key),
                        ));
                    }
                }
            }
        }

        // Invariant B: If effective backend is not DXVK, there must be no DXVK-forcing overrides or DXVK-only DLL path injection.
        if ctx.graphics_stack.effective_backend != "DXVK" {
            if let Some(spec) = &ctx.command_spec {
                if let Some(overrides) = spec.env.get("WINEDLLOVERRIDES") {
                    let dxvk_dlls = ["d3d8", "d3d9", "d3d10core", "d3d11", "dxgi"];
                    for part in overrides.split(';') {
                        if let Some((dll, mode)) = part.split_once('=') {
                            let dll_trimmed = dll.trim().to_lowercase();
                            if dxvk_dlls.contains(&dll_trimmed.as_str()) && mode.contains('n') {
                                warnings.push((
                                    "INVARIANT_B_VIOLATION",
                                    format!("Effective backend is not DXVK but found native override for DXVK DLL: {}", dll),
                                ));
                            }
                        }
                    }
                }
                if let Some(dll_path) = spec.env.get("WINEDLLPATH") {
                    if dll_path.contains("dxvk") {
                        warnings.push((
                            "INVARIANT_B_VIOLATION",
                            "Effective backend is not DXVK but WINEDLLPATH contains 'dxvk'".into(),
                        ));
                    }
                }
            }
        }

        // Invariant C: If effective D3D12 provider is unset/default/not-selected, there must be no forced D3D12 provider injection.
        if ctx.graphics_stack.effective_d3d12_provider == "None" {
            if let Some(spec) = &ctx.command_spec {
                if let Some(overrides) = spec.env.get("WINEDLLOVERRIDES") {
                    let d3d12_dlls = ["d3d12", "d3d12core"];
                    for part in overrides.split(';') {
                        if let Some((dll, mode)) = part.split_once('=') {
                            let dll_trimmed = dll.trim().to_lowercase();
                            if d3d12_dlls.contains(&dll_trimmed.as_str()) && mode.contains('n') {
                                warnings.push((
                                    "INVARIANT_C_VIOLATION",
                                    format!("Effective D3D12 provider is None but found native override for DLL: {}", dll),
                                ));
                            }
                        }
                    }
                }
                if let Some(dll_path) = spec.env.get("WINEDLLPATH") {
                    if dll_path.contains("vkd3d") {
                        warnings.push((
                            "INVARIANT_C_VIOLATION",
                            "Effective D3D12 provider is None but WINEDLLPATH contains 'vkd3d'".into(),
                        ));
                    }
                }
            }
        }

        // Detailed Invariant C: Effective D3D12 provider must match resolved provider paths if one is active.
        // If effective is "vkd3d-proton", we expect to see it in the path.
        // If effective is "vkd3d", we expect to see vkd3d but NOT vkd3d-proton in the path.
        if ctx.graphics_stack.effective_d3d12_provider != "None" {
             let provider = &ctx.graphics_stack.effective_d3d12_provider;
             for res in &ctx.dll_resolutions {
                 if res.name == "d3d12" || res.name == "d3d12core" {
                     if let Some(path) = &res.chosen_path {
                         let path_str = path.to_string_lossy().to_lowercase();
                         if provider == "vkd3d-proton" && !path_str.contains("vkd3d-proton") && path_str.contains("vkd3d") {
                              warnings.push((
                                 "INVARIANT_C_MISMATCH",
                                 format!("Effective provider is vkd3d-proton but resolved path points to plain vkd3d: {}", path.display())
                             ));
                         } else if provider == "vkd3d" && path_str.contains("vkd3d-proton") {
                              warnings.push((
                                 "INVARIANT_C_MISMATCH",
                                 format!("Effective provider is vkd3d but resolved path points to vkd3d-proton: {}", path.display())
                             ));
                         }
                     }
                 }
             }
        }

        // Invariant D: explicit user setting must not be silently overwritten
        // ONLY RUN if effective backend is populated (non-empty)
        if !ctx.graphics_stack.effective_backend.is_empty() && !ctx.graphics_stack.requested_backend.is_empty() && ctx.graphics_stack.requested_backend != "Auto" {
            // requested_backend is likely Debug string of enum, e.g. "DXVK" or "WineD3D"
            let req = &ctx.graphics_stack.requested_backend;
            let eff = &ctx.graphics_stack.effective_backend;

            let mismatch = (req == "DXVK" && eff != "DXVK") || (req == "WineD3D" && eff != "WineD3D (Baseline)");

            if mismatch {
                let reason = ctx.graphics_stack.fallback_reasons.get("graphics_backend").cloned().unwrap_or_else(|| "unknown".into());
                let code = if req == "DXVK" || req == "WineD3D" {
                    "STRICT_POLICY_VIOLATION"
                } else {
                    "INVARIANT_D_BACKEND_MISMATCH"
                };
                warnings.push((
                    code,
                    format!("Requested backend '{}' differs from effective '{}'. Reason: {}",
                        ctx.graphics_stack.requested_backend, ctx.graphics_stack.effective_backend, reason)
                ));
            }
        }

        if !ctx.graphics_stack.effective_d3d12_provider.is_empty() && !ctx.graphics_stack.requested_d3d12_provider.is_empty() && ctx.graphics_stack.requested_d3d12_provider != "Auto" {
            if ctx.graphics_stack.requested_d3d12_provider != ctx.graphics_stack.effective_d3d12_provider {
                 let reason = ctx.graphics_stack.fallback_reasons.get("d3d12_provider").cloned().unwrap_or_else(|| "unknown".into());
                 warnings.push((
                    "INVARIANT_D_D3D12_MISMATCH",
                    format!("Requested D3D12 provider '{}' differs from effective '{}'. Reason: {}",
                        ctx.graphics_stack.requested_d3d12_provider, ctx.graphics_stack.effective_d3d12_provider, reason)
                ));
            }
        }

        if let (Some(requested_gpu), Some(effective_gpu_val)) = (&ctx.graphics_stack.requested_gpu, &ctx.graphics_stack.effective_gpu) {
            if effective_gpu_val.is_empty() { return; }
            let mut mismatch = true;
            if let Some(effective_gpu) = &ctx.graphics_stack.effective_gpu {
                 // Check for partial match since effective names are synthesized
                 if effective_gpu.contains(requested_gpu) || requested_gpu.contains("NVIDIA") && effective_gpu.contains("NVIDIA") {
                     mismatch = false;
                 }
            }

            if mismatch {
                let reason = ctx.graphics_stack.fallback_reasons.get("gpu").cloned().unwrap_or_else(|| "unknown".into());
                warnings.push((
                    "INVARIANT_D_GPU_MISMATCH",
                    format!("Requested GPU '{}' differs from effective '{:?}'. Reason: {}",
                        requested_gpu, ctx.graphics_stack.effective_gpu, reason)
                ));
            }
        }

        // Post-launch validation: Check for missing evidence after launch
        if ctx.graphics_stack.effective_backend == "DXVK" && !ctx.graphics_stack.runtime_evidence.dxvk.evidence_found {
             let meta = &ctx.graphics_stack.runtime_evidence.scan_metadata;
             let suffix = if !meta.file_exists { " (Wine log missing)" } else if meta.line_count == 0 { " (Wine log empty)" } else { "" };
             let code = if ctx.graphics_stack.requested_backend == "DXVK" {
                 "STRICT_DXVK_EVIDENCE_MISSING"
             } else {
                 "DIAGNOSTICS_MISSING_DXVK_EVIDENCE"
             };

             let msg = if ctx.verification.status == "failed_after_spawn" {
                 format!("DXVK was requested/effective but the game process exited immediately (lifetime: {}ms, exit code: {:?}). No runtime evidence could be collected.",
                     ctx.verification.process_lifetime_ms.unwrap_or(0), ctx.verification.exit_code)
             } else {
                 format!("DXVK was requested/effective but no runtime evidence was found in logs{}. This may indicate a silent fallback to WineD3D.", suffix)
             };

             warnings.push((code, msg));
        }

        // Post-launch validation: Early process death
        if ctx.verification.status == "failed_after_spawn" {
             warnings.push((
                "LAUNCH_EARLY_EXIT",
                format!("The game process exited immediately after spawn (exit code: {:?}, lifetime: {}ms).",
                    ctx.verification.exit_code, ctx.verification.process_lifetime_ms.unwrap_or(0))
            ));
        } else if ctx.verification.status == "uncertain" {
             warnings.push((
                "LAUNCH_UNCERTAIN",
                "The launch status is uncertain: the process is running but no meaningful log growth or graphics evidence was observed within the verification window.".into()
            ));
        }

        // Additional check for WineD3D usage when DXVK was requested
        if ctx.graphics_stack.requested_backend == "DXVK" && ctx.graphics_stack.runtime_evidence.wined3d.evidence_found {
            warnings.push((
                "STRICT_DXVK_POLICY_VIOLATION",
                "DXVK was explicitly requested, but WineD3D usage was observed in logs.".into(),
            ));
        }

        // Check for DLL Load Failures in strict mode
        if !ctx.graphics_stack.requested_backend.is_empty() && ctx.graphics_stack.requested_backend != "Auto" {
            for evidence in &ctx.graphics_stack.graphics_stack_evidence {
                if evidence.contains("DLL Load Failure") || evidence.contains("DLL Dependency Missing") {
                    let code = if ctx.graphics_stack.requested_backend == "DXVK" { "STRICT_DXVK_LOAD_FAILURE" } else { "STRICT_POLICY_LOAD_FAILURE" };

                    // Try to extract DLL name for more precise error reporting
                    let dll_name = if evidence.contains("L\"") {
                         evidence.split("L\"").nth(1).and_then(|s| s.split('\"').next()).unwrap_or("unknown")
                    } else if evidence.contains("Library ") {
                         evidence.split("Library ").nth(1).and_then(|s| s.split(' ').next()).unwrap_or("unknown")
                    } else {
                         "unknown"
                    };

                    // Noise filter for common non-fatal middleware/bootstrap DLLs
                    if dll_name.contains("winemac.drv") || dll_name.contains("steam_api") {
                        continue;
                    }

                    let stem = dll_name.trim_end_matches(".dll");
                    let resolution = ctx.dll_resolutions.iter().find(|r| r.name == stem || r.name == dll_name);
                    let provider = resolution.map(|r| format!("{:?}", r.chosen_provider)).unwrap_or_else(|| "Unknown".into());
                    let path = resolution.and_then(|r| r.chosen_path.as_ref()).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| "Unknown".into());

                    warnings.push((
                        code,
                        format!("Strict policy violation: a required native DLL failed to load: {}. [DLL: {}, Provider: {}, Path: {}, Arch: {:?}]",
                            evidence, dll_name, provider, path, ctx.graphics_stack.target_architecture),
                    ));
                }
            }
        }

        if ctx.graphics_stack.effective_d3d12_provider == "vkd3d-proton" && !ctx.graphics_stack.runtime_evidence.vkd3d_proton.evidence_found {
             let meta = &ctx.graphics_stack.runtime_evidence.scan_metadata;
             let suffix = if !meta.file_exists { " (Wine log missing)" } else if meta.line_count == 0 { " (Wine log empty)" } else { "" };
             warnings.push((
                "DIAGNOSTICS_MISSING_VKD3D_PROTON_EVIDENCE",
                format!("VKD3D-Proton was requested/effective but no runtime evidence was found in logs{}.", suffix)
            ));
        }


        for (code, msg) in warnings {
            ctx.add_warning(code, msg);
        }
    }
}
