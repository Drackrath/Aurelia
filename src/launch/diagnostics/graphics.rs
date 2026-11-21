//! Graphics stack evidence and reporting.

use crate::launch::context::*;
use crate::launch::pipeline::LaunchPipeline;
use std::collections::HashMap;

impl LaunchPipeline {
    pub(crate) fn populate_effective_graphics_stack(&self, ctx: &mut PipelineContext) {
        if let Some(spec) = &ctx.command_spec {
            if let Some(overrides) = spec.env.get("WINEDLLOVERRIDES") {
                let has_dxvk = overrides.contains("d3d11=n")
                    || overrides.contains("dxgi=n")
                    || overrides.contains("d3d9=n")
                    || overrides.contains("d3d8=n");
                let has_vkd3dp = overrides.contains("d3d12=n");
                let has_vkd3dw = overrides.contains("libvkd3d-1=n");

                ctx.graphics_stack.effective_backend = if has_dxvk {
                    "DXVK"
                } else {
                    "WineD3D (Baseline)"
                }
                .to_string();
                ctx.graphics_stack.effective_d3d12_provider = if has_vkd3dp {
                    "vkd3d-proton"
                } else if has_vkd3dw {
                    "vkd3d"
                } else {
                    "None"
                }
                .to_string();

                if has_dxvk || has_vkd3dp || has_vkd3dw {
                    ctx.graphics_stack.override_policy = "Native-preferred".to_string();
                } else {
                    ctx.graphics_stack.override_policy = "Builtin-only".to_string();
                }
            } else {
                ctx.graphics_stack.effective_backend = "WineD3D (Baseline)".to_string();
                ctx.graphics_stack.effective_d3d12_provider = "None".to_string();
                ctx.graphics_stack.override_policy = "Builtin-only".to_string();
            }

            // Synchronize expected stack string with effective resolution
            let mut stack_parts = Vec::new();
            if ctx.graphics_stack.effective_backend == "DXVK" {
                stack_parts.push("DXVK");
            }
            if ctx.graphics_stack.effective_d3d12_provider != "None" {
                stack_parts.push(&ctx.graphics_stack.effective_d3d12_provider);
            }

            if stack_parts.is_empty() {
                ctx.graphics_stack.graphics_stack_expected = "WineD3D (Baseline)".to_string();
            } else {
                ctx.graphics_stack.graphics_stack_expected = stack_parts.join(", ");
            }

            let nvapi_res = ctx.dll_resolutions.iter().find(|r| r.name == "nvapi");
            ctx.graphics_stack.effective_nvapi = nvapi_res
                .map(|r| {
                    r.chosen_provider != crate::launch::dll_provider_resolver::DllProvider::None
                })
                .unwrap_or(false);

            // GPU Selection
            ctx.graphics_stack.effective_gpu = None;
            if let Some(val) = spec.env.get("__NV_PRIME_RENDER_OFFLOAD") {
                if val == "1" {
                    ctx.graphics_stack.effective_gpu = Some("NVIDIA Discrete GPU".to_string());
                }
            } else if let Some(val) = spec.env.get("DRI_PRIME") {
                if val == "1" {
                    ctx.graphics_stack.effective_gpu =
                        Some("Secondary GPU (DRI_PRIME=1)".to_string());
                } else if val == "0" {
                    ctx.graphics_stack.effective_gpu =
                        Some("Primary GPU (DRI_PRIME=0)".to_string());
                }
            }
        }
    }

    pub(crate) fn populate_expected_graphics_stack(&self, ctx: &mut PipelineContext) {
        if let Some(config) = &ctx.user_config {
            ctx.graphics_stack.requested_backend =
                format!("{:?}", config.graphics_layers.graphics_backend_policy);
            ctx.graphics_stack.requested_d3d12_provider =
                format!("{:?}", config.graphics_layers.d3d12_policy);
            ctx.graphics_stack.requested_nvapi = config.graphics_layers.nvapi_enabled;
            ctx.graphics_stack.requested_gpu = config.gpu_preference.clone();

            // Initial baseline assumption - will be overridden by populate_effective_graphics_stack
            ctx.graphics_stack.graphics_stack_expected = "WineD3D (Baseline)".to_string();
            ctx.graphics_stack.override_policy = "Builtin-only".to_string();
        }
    }

    pub(crate) fn record_dll_provider_diagnostics(&self, ctx: &mut PipelineContext) {
        for res in &ctx.dll_resolutions {
            ctx.graphics_stack
                .dll_providers
                .insert(res.name.clone(), format!("{:?}", res.chosen_provider));
            if let Some(reason) = &res.fallback_reason {
                ctx.graphics_stack
                    .fallback_reasons
                    .insert(res.name.clone(), reason.clone());
            }
        }
    }

    pub(crate) async fn scan_logs_for_graphics_evidence(&self, ctx: &mut PipelineContext) {
        let scan_start = std::time::Instant::now();
        if let Some(session) = &ctx.session {
            let log_path = ctx
                .command_spec
                .as_ref()
                .and_then(|spec| spec.env.get("WINE_LOG_OUTPUT"))
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| session.stderr_path());
            ctx.graphics_stack.runtime_evidence.scan_metadata.log_path =
                log_path.to_string_lossy().to_string();

            let mut retries = 0;
            let max_retries = 3;
            let mut content = String::new();

            let initial_file_size = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);

            while retries <= max_retries {
                if log_path.exists() {
                    ctx.graphics_stack
                        .runtime_evidence
                        .scan_metadata
                        .file_exists = true;
                    let current_size = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
                    ctx.graphics_stack.runtime_evidence.scan_metadata.file_size = current_size;

                    if current_size > initial_file_size {
                        ctx.verification.log_growth_observed = true;
                    }

                    if let Ok(current_content) = std::fs::read_to_string(&log_path) {
                        if !current_content.is_empty() || retries == max_retries {
                            content = current_content;
                            break;
                        }
                    }
                }

                let process_running = if let Some(child) = &mut ctx.child {
                    child.try_wait().map(|s| s.is_none()).unwrap_or(false)
                } else {
                    false
                };
                ctx.graphics_stack
                    .runtime_evidence
                    .scan_metadata
                    .process_running_during_scan = process_running;

                if !process_running && retries > 0 {
                    break;
                }

                retries += 1;
                ctx.graphics_stack.runtime_evidence.scan_metadata.retries = retries;
                tokio::time::sleep(std::time::Duration::from_millis(200 * retries as u64)).await;
            }

            if !content.is_empty() {
                let lines: Vec<&str> = content.lines().collect();
                ctx.graphics_stack.runtime_evidence.scan_metadata.line_count = lines.len();

                // Capture Log Head/Tail
                ctx.verification.log_head = lines.iter().take(50).map(|s| s.to_string()).collect();
                if lines.len() > 50 {
                    ctx.verification.log_tail = lines
                        .iter()
                        .rev()
                        .take(100)
                        .rev()
                        .map(|s| s.to_string())
                        .collect();
                }

                let mut max_milestone = crate::logging::StartupMilestone::None;

                // Derive component paths from dll resolutions
                let mut component_paths = HashMap::new();
                for res in &ctx.dll_resolutions {
                    if res.chosen_provider
                        == crate::launch::dll_provider_resolver::DllProvider::Runner
                    {
                        if let Some(path) = &res.chosen_path {
                            if let Some(parent) = path.parent() {
                                let family =
                                    if res.name.contains("d3d12") || res.name.contains("vkd3d") {
                                        if path.to_string_lossy().contains("vkd3d-proton") {
                                            "vkd3d_proton"
                                        } else {
                                            "vkd3d"
                                        }
                                    } else {
                                        "dxvk"
                                    };
                                component_paths
                                    .insert(family, parent.to_string_lossy().to_string());
                            }
                        }
                    }
                }

                for line in &lines {
                    let mut line_matched = false;
                    let line_lower = line.to_lowercase();

                    // Detect Dependency Families from log lines
                    let families_to_scan = [
                        ("Batman (GFSDK/APEX)", vec!["gfsdk", "apex", "nvtt"]),
                        ("Metro (4A Engine)", vec!["4a engine", "metro"]),
                        ("PhysX Runtime", vec!["physx"]),
                    ];

                    for (family, patterns) in families_to_scan {
                        if patterns.iter().any(|p| line_lower.contains(p))
                            && !ctx
                                .verification
                                .dependency_families_detected
                                .contains(&family.to_string())
                        {
                            ctx.verification
                                .dependency_families_detected
                                .push(family.to_string());
                        }
                    }

                    // Update Milestone
                    if let Some(m) = crate::logging::detect_startup_milestone(line) {
                        if m > max_milestone {
                            max_milestone = m;
                        }
                    }

                    if let Some(evidence) = crate::logging::classify_graphics_evidence(line) {
                        if !ctx
                            .graphics_stack
                            .graphics_stack_evidence
                            .contains(&evidence)
                        {
                            ctx.graphics_stack
                                .graphics_stack_evidence
                                .push(evidence.clone());
                        }

                        if evidence.contains("SteamAPI Initialization Failed") {
                            ctx.verification.steam_api_initialized = Some(false);
                        }
                        if evidence.contains("Steam Ownership Validation Failed") {
                            ctx.verification.steam_ownership_confirmed = Some(false);
                        }
                        if evidence.contains("Steam Client Artifact:") {
                            if evidence.contains("local") {
                                ctx.verification.steam_client_artifact = Some("local".into());
                            } else if evidence.contains("windows") {
                                ctx.verification.steam_client_artifact = Some("windows".into());
                            } else if evidence.contains("host") {
                                ctx.verification.steam_client_artifact = Some("host".into());
                            }
                        }

                        if evidence.contains("DXVK") {
                            record_evidence(
                                &mut ctx.graphics_stack.runtime_evidence.dxvk,
                                evidence.clone(),
                            );
                        }
                        if evidence.contains("VKD3D-Proton") {
                            record_evidence(
                                &mut ctx.graphics_stack.runtime_evidence.vkd3d_proton,
                                evidence.clone(),
                            );
                        }
                        if evidence.contains("VKD3D") && !evidence.contains("Proton") {
                            record_evidence(
                                &mut ctx.graphics_stack.runtime_evidence.vkd3d,
                                evidence.clone(),
                            );
                        }
                        if evidence.contains("WineD3D") {
                            record_evidence(
                                &mut ctx.graphics_stack.runtime_evidence.wined3d,
                                evidence.clone(),
                            );
                        }
                        line_matched = true;
                    }

                    // Robust Path & Module Matching

                    // DXVK Path Match
                    if let Some(path) = component_paths.get("dxvk") {
                        if line_lower.contains(&path.to_lowercase()) {
                            record_evidence(
                                &mut ctx.graphics_stack.runtime_evidence.dxvk,
                                format!("Path match: {}", line.trim()),
                            );
                            line_matched = true;
                        }
                    }

                    // VKD3D-Proton Path Match
                    if let Some(path) = component_paths.get("vkd3d_proton") {
                        if line_lower.contains(&path.to_lowercase()) {
                            record_evidence(
                                &mut ctx.graphics_stack.runtime_evidence.vkd3d_proton,
                                format!("Path match: {}", line.trim()),
                            );
                            line_matched = true;
                        }
                    }

                    // Generic Module Loader matches for graphics DLLs
                    if line_lower.contains("loaddll")
                        || line_lower.contains("load_module")
                        || line_lower.contains("import_dll")
                    {
                        let graphics_dlls = [
                            "d3d9",
                            "d3d11",
                            "dxgi",
                            "d3d12",
                            "d3d8",
                            "d3d10core",
                            "libvkd3d",
                        ];
                        for dll in graphics_dlls {
                            if line_lower.contains(dll) {
                                let is_failed =
                                    line_lower.contains("failed") || line_lower.contains("error");
                                let is_builtin = line_lower.contains("system32")
                                    || (line_lower.contains("builtin")
                                        && !line_lower.contains("native"));

                                let evidence_type = if is_failed {
                                    "Module load FAILED"
                                } else if is_builtin {
                                    "Module load (builtin/fallback)"
                                } else {
                                    "Module load (likely native)"
                                };

                                let msg = format!("{}: {}", evidence_type, line.trim());
                                if !ctx.graphics_stack.graphics_stack_evidence.contains(&msg) {
                                    ctx.graphics_stack.graphics_stack_evidence.push(msg.clone());

                                    if is_builtin {
                                        record_evidence(
                                            &mut ctx.graphics_stack.runtime_evidence.wined3d,
                                            msg,
                                        );
                                    }
                                }
                                line_matched = true;
                            }
                        }
                    }

                    if line_matched {
                        ctx.graphics_stack
                            .runtime_evidence
                            .scan_metadata
                            .candidate_matches += 1;
                    }
                }

                ctx.verification.last_successful_startup_milestone = max_milestone.to_string();
            }
        } else {
            ctx.graphics_stack
                .runtime_evidence
                .scan_metadata
                .file_exists = false;
        }

        // Populate expectations
        ctx.graphics_stack.runtime_evidence.dxvk.expected =
            ctx.graphics_stack.effective_backend == "DXVK";
        ctx.graphics_stack.runtime_evidence.vkd3d_proton.expected =
            ctx.graphics_stack.effective_d3d12_provider == "vkd3d-proton";
        ctx.graphics_stack.runtime_evidence.vkd3d.expected =
            ctx.graphics_stack.effective_d3d12_provider == "vkd3d";
        ctx.graphics_stack.runtime_evidence.wined3d.expected =
            ctx.graphics_stack.effective_backend == "WineD3D (Baseline)";

        // Build diagnosis
        let diagnose = |item: &mut EvidenceItem, name: &str, metadata: &EvidenceScanMetadata| {
            if !metadata.file_exists {
                item.diagnosis = format!("{} requested; Wine log missing", name);
                item.diagnostics_note = format!("Log file was not found at {}.", metadata.log_path);
                return;
            }
            if metadata.line_count == 0 {
                if metadata.process_running_during_scan {
                    item.diagnosis = format!("{} requested; Wine log empty (early scan)", name);
                    item.diagnostics_note = format!(
                        "Log file exists but is empty. Process is still running (retries: {}). Evidence may appear later.",
                        metadata.retries
                    );
                } else {
                    item.diagnosis = format!("{} requested; Wine log empty", name);
                    item.diagnostics_note = format!(
                        "Log file exists but contains no lines after {} retries. Process has exited. Logging may be disabled or failed to initialize.",
                        metadata.retries
                    );
                }
                return;
            }

            if item.expected {
                if item.evidence_found {
                    item.diagnosis =
                        format!("confirmed: {} loaded based on runtime evidence", name);
                } else {
                    item.diagnosis = format!(
                        "inconclusive: {} requested but no runtime evidence found",
                        name
                    );
                    item.diagnostics_note = format!(
                        "Scanned {} lines, found {} matches for graphics patterns, but none confirmed {} usage.",
                        metadata.line_count, metadata.candidate_matches, name
                    );
                }
            } else if item.evidence_found {
                item.diagnosis = format!(
                    "suspected fallback: {} not requested but runtime evidence found",
                    name
                );
            } else {
                item.diagnosis = "neutral: inconclusive".to_string();
            }
        };

        let meta = ctx.graphics_stack.runtime_evidence.scan_metadata.clone();
        diagnose(&mut ctx.graphics_stack.runtime_evidence.dxvk, "DXVK", &meta);
        diagnose(
            &mut ctx.graphics_stack.runtime_evidence.vkd3d_proton,
            "VKD3D-Proton",
            &meta,
        );
        diagnose(
            &mut ctx.graphics_stack.runtime_evidence.vkd3d,
            "VKD3D",
            &meta,
        );
        diagnose(
            &mut ctx.graphics_stack.runtime_evidence.wined3d,
            "WineD3D",
            &meta,
        );

        // Refinement: Do not mark WineD3D fallback when primary components are confirmed
        if (ctx.graphics_stack.runtime_evidence.dxvk.evidence_found
            || ctx
                .graphics_stack
                .runtime_evidence
                .vkd3d_proton
                .evidence_found
            || ctx.graphics_stack.runtime_evidence.vkd3d.evidence_found)
            && !ctx.graphics_stack.runtime_evidence.wined3d.expected
        {
            if ctx.graphics_stack.runtime_evidence.wined3d.evidence_found {
                let has_strong_wined3d = ctx
                    .graphics_stack
                    .runtime_evidence
                    .wined3d
                    .evidence
                    .iter()
                    .any(|e| e.contains("Fallback Detected"));
                if !has_strong_wined3d {
                    ctx.graphics_stack.runtime_evidence.wined3d.diagnosis =
                        "neutral: inconclusive (standard builtins loaded)".to_string();
                }
            }
        }

        // Update confidence based on evidence vs expectation
        if ctx.graphics_stack.graphics_stack_evidence.is_empty() {
            ctx.graphics_stack.graphics_stack_confidence = "low".to_string();
        } else {
            let has_expected = !ctx.graphics_stack.runtime_evidence.dxvk.expected
                || ctx.graphics_stack.runtime_evidence.dxvk.evidence_found;

            if has_expected {
                ctx.graphics_stack.graphics_stack_confidence = "high".to_string();
            } else {
                ctx.graphics_stack.graphics_stack_confidence = "medium".to_string();
            }
        }
        ctx.graphics_stack
            .runtime_evidence
            .scan_metadata
            .scan_duration_ms = scan_start.elapsed().as_millis();
    }
}

/// Mark an evidence item as found and record the supporting line, capping the
/// stored evidence at 5 entries to bound memory/serialized size.
fn record_evidence(item: &mut EvidenceItem, line: String) {
    item.evidence_found = true;
    if item.evidence.len() < 5 {
        item.evidence.push(line);
    }
}
