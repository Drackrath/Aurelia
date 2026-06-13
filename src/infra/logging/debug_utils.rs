use crate::infra::logging::{LaunchSummary, EffectiveEnv};
use anyhow::{Context, Result};
use std::path::Path;

pub fn load_launch_summary(log_dir: &Path) -> Result<LaunchSummary> {
    let summary_path = log_dir.join("summary.json");
    let content = std::fs::read_to_string(&summary_path)
        .with_context(|| format!("Failed to read summary at {}", summary_path.display()))?;
    Ok(serde_json::from_str(&content)?)
}

pub fn load_effective_env(log_dir: &Path) -> Result<EffectiveEnv> {
    let env_path = log_dir.join("effective_env.json");
    let content = std::fs::read_to_string(&env_path)
        .with_context(|| format!("Failed to read effective env at {}", env_path.display()))?;
    Ok(serde_json::from_str(&content)?)
}

pub fn print_launch_summary(summary: &LaunchSummary) {
    println!("--- Launch Summary ---");
    println!("Session ID    : {}", summary.session_id);
    println!("App           : {} ({})", summary.app_name.as_deref().unwrap_or("Unknown"), summary.app_id);
    println!("Result        : {:?}", summary.result);
    if let Some(stage) = &summary.failing_stage {
        println!("Failing Stage : {}", stage);
    }
    println!("Total Duration: {}ms", summary.total_duration_ms);
    println!("Stage Timings :");
    let mut sorted_stages: Vec<_> = summary.stage_durations_ms.iter().collect();
    sorted_stages.sort_by_key(|(_, d)| std::cmp::Reverse(**d));
    for (stage, duration) in sorted_stages {
        println!("  - {}: {}ms", stage, duration);
    }
    if !summary.warnings.is_empty() {
        println!("Warnings      :");
        for warning in &summary.warnings {
            println!("  [{}] {}", warning.code, warning.message);
        }
    }
    println!("----------------------");
}

pub fn print_effective_env(env: &EffectiveEnv) {
    println!("--- Effective Environment ---");
    println!("Runner      : {}", env.runner_name);
    if let Some(overrides) = &env.wine_dll_overrides {
        println!("DLL Overrides: {}", overrides);
    }
    println!("Environment Variables:");
    let mut sorted_vars: Vec<_> = env.env_vars.iter().collect();
    sorted_vars.sort_by_key(|(key, _)| *key);
    for (key, value) in sorted_vars {
        println!("  {}={}", key, value);
    }
    println!("-----------------------------");
}
