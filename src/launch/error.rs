//! Launch error kinds and mapping.

use std::fmt;

use crate::launch::context::*;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LaunchErrorKind {
    Validation,  // Missing app context, invalid app id
    Environment, // Missing proton path, config issues
    Permission,  // Failed to create dir, set permissions
    Runner,      // Runner prepare_prefix/build_command failed
    GameData,    // appmanifest not found, executable not found
    Process,     // Failed to spawn process
    Dependency,  // Missing MangoHud, etc.
    Unknown,
}

impl fmt::Display for LaunchErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Validation => "Validation",
            Self::Environment => "Environment",
            Self::Permission => "Permission",
            Self::Runner => "Runner",
            Self::GameData => "Game Data",
            Self::Process => "Process",
            Self::Dependency => "Dependency",
            Self::Unknown => "Unknown",
        };
        write!(f, "{}", s)
    }
}

impl LaunchErrorKind {
    pub fn remediation_hint(&self) -> &'static str {
        match self {
            Self::Validation => "Check if the game is correctly imported.",
            Self::Environment => "Verify your Global Settings and Compatibility Layer path.",
            Self::Permission => "Check filesystem permissions for the library and prefix folders.",
            Self::Runner => "Try a different Proton/Wine version.",
            Self::GameData => "Verify integrity of game files or reinstall the game.",
            Self::Process => "Check if the game is already running or if files are locked.",
            Self::Dependency => "Install missing system dependencies.",
            Self::Unknown => "Check the detailed logs for more information.",
        }
    }
}

#[derive(Debug)]
pub struct LaunchError {
    pub kind: LaunchErrorKind,
    pub message: String,
    pub context: HashMap<String, String>,
    pub source: Option<anyhow::Error>,
}

impl LaunchError {
    pub fn new(kind: LaunchErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            context: HashMap::new(),
            source: None,
        }
    }

    pub fn with_context(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }

    pub fn with_source(mut self, source: anyhow::Error) -> Self {
        self.source = Some(source);
        self
    }
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {} - {}",
            self.kind,
            self.message,
            self.kind.remediation_hint()
        )
    }
}

impl std::error::Error for LaunchError {}

pub fn map_io_error(err: &std::io::Error, dup_info: Option<&DuplicateInstanceInfo>) -> LaunchError {
    use std::io::ErrorKind;

    let kind = match err.kind() {
        ErrorKind::NotFound => LaunchErrorKind::GameData,
        ErrorKind::PermissionDenied => LaunchErrorKind::Permission,
        ErrorKind::InvalidInput => LaunchErrorKind::Validation,
        _ => LaunchErrorKind::Process,
    };

    let mut message = match err.kind() {
        ErrorKind::NotFound => "The game executable or runner was not found.".to_string(),
        ErrorKind::PermissionDenied => {
            "Access denied while attempting to start the game. Check file permissions.".to_string()
        }
        ErrorKind::InvalidInput => "Invalid launch configuration or malformed path.".to_string(),
        ErrorKind::AlreadyExists => {
            "A lock file or another instance of the game was detected.".to_string()
        }
        _ => format!("Process spawn failed: {}", err),
    };

    // ONLY show duplicate instance hint if explicitly detected or AlreadyExists kind
    let err_str = err.to_string();
    let lock_msg = "Game files are locked by another process. Ensure no other instance is running.";

    if let Some(info) = dup_info {
        if info.detected {
            message = lock_msg.to_string();
        }
    } else if err.kind() == ErrorKind::AlreadyExists
        || err_str.contains("locked")
        || err_str.contains("Resource busy")
    {
        message = lock_msg.to_string();
    }

    let mut launch_err =
        LaunchError::new(kind, message).with_context("io_kind", format!("{:?}", err.kind()));

    if let Some(code) = err.raw_os_error() {
        launch_err = launch_err.with_context("os_errno", code.to_string());
    }

    let (detected, source) = match dup_info {
        Some(info) => (info.detected.to_string(), info.source.clone()),
        None => ("false".to_string(), "none".to_string()),
    };
    launch_err = launch_err.with_context("duplicate_instance_detected", detected);
    launch_err = launch_err.with_context("duplicate_detection_source", source);

    launch_err
}

/// Mark an evidence item as found and record the supporting line, capping the
/// stored evidence at 5 entries to bound memory/serialized size.

#[derive(Debug)]
pub struct PipelineError {
    pub stage_name: String,
    pub inner: LaunchError,
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Pipeline failed at stage '{}': {}",
            self.stage_name, self.inner
        )
    }
}

impl std::error::Error for PipelineError {}

#[cfg(test)]
mod tests;
