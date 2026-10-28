//! `capabilities`: what this build supports.

use clap::CommandFactory;
use serde_json::{json, Map, Value};

use aurelia::core::error::ErrorKind;

use crate::cli::Cli;
use crate::commands::common::*;

/// Every `event` name in JSON output.
pub(crate) const EVENTS: &[&str] = &[
    "awaiting_confirmation",
    "closed",
    "echo",
    "error",
    "guard_required",
    "message",
    "not_found",
    "openid_challenge",
    "progress",
    "qr_challenge",
    "qr_scanned",
    "ready",
    "result",
    "stopping",
    "typing",
    "web_token_required",
];

/// Behaviours the flags don't reveal.
pub(crate) const FEATURES: &[&str] = &[
    // One typed result line per outcome.
    "login.qr.result_line",
    // stdin `cancel` or EOF ends it.
    "login.qr.stdin_control",
    // `result` lines carry `schema`.
    "result.schema",
];

pub(crate) fn cmd_capabilities(json: bool) {
    let report = capabilities();
    if json {
        print_json_line(&report);
        return;
    }
    let commands = report["commands"].as_object().map_or(0, Map::len);
    cli_println!(
        "aurelia {} ({} {}), JSON schema {JSON_SCHEMA}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    cli_println!("Features: {}", FEATURES.join(", "));
    cli_println!("Commands: {commands} (run `aurelia --json capabilities` for flags, events and errors)");
}

/// The full capability document.
pub(crate) fn capabilities() -> Value {
    let root = Cli::command();
    let commands: Map<String, Value> = root
        .get_subcommands()
        .filter(|c| !c.is_hide_set())
        .map(|c| (c.get_name().to_string(), describe(c)))
        .collect();
    let errors: Vec<Value> = ErrorKind::ALL
        .iter()
        .map(|k| json!({ "type": k, "exit_code": k.exit_code(), "hint": k.hint() }))
        .collect();
    json!({
        "name": "aurelia",
        "version": env!("CARGO_PKG_VERSION"),
        "schema": JSON_SCHEMA,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "global_flags": long_flags(&root),
        "commands": commands,
        "events": EVENTS,
        "errors": errors,
        "features": FEATURES,
    })
}

/// One command's flags, args, subcommands.
fn describe(cmd: &clap::Command) -> Value {
    let args: Vec<&str> = cmd
        .get_positionals()
        .filter(|a| !a.is_hide_set())
        .map(|a| a.get_id().as_str())
        .collect();
    let mut value = json!({ "flags": long_flags(cmd), "args": args });
    let subs: Map<String, Value> = cmd
        .get_subcommands()
        .filter(|c| !c.is_hide_set())
        .map(|c| (c.get_name().to_string(), describe(c)))
        .collect();
    if !subs.is_empty() {
        value["subcommands"] = Value::Object(subs);
    }
    value
}

/// Visible `--long` flags of a command.
fn long_flags(cmd: &clap::Command) -> Vec<String> {
    cmd.get_arguments()
        .filter(|a| !a.is_hide_set() && !a.is_positional())
        .filter_map(|a| a.get_long().map(|l| format!("--{l}")))
        .collect()
}

#[cfg(test)]
#[path = "capabilities_tests.rs"]
mod tests;
