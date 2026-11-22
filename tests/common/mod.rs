//! Helpers shared by integration tests.
#![allow(dead_code)]

use std::path::Path;
use std::process::{Command, Output};

/// Run the binary against an isolated config.
pub fn aurelia(config_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aurelia"))
        .args(args)
        .env("AURELIA_CONFIG_DIR", config_dir)
        .env("AURELIA_NO_DAEMON", "1")
        .output()
        .expect("failed running the aurelia binary")
}

/// Path as a quoted JSON string.
pub fn json_path(p: &Path) -> String {
    serde_json::to_string(&p.to_string_lossy()).unwrap()
}

/// Path with VDF backslash escaping.
pub fn vdf_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "\\\\")
}

/// Captured stderr as text.
pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// Stdout then stderr as text.
pub fn out(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}
