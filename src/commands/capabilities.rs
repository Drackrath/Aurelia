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
    } else {
        cli_print!("{}", render_report(&report, terminal_width()));
    }
}

/// Labels for the list view.
const LIST_FIELDS: &[(&str, &str)] = &[
    ("Name", "name"),
    ("Version", "version"),
    ("Schema", "schema"),
    ("OS", "os"),
    ("Arch", "arch"),
    ("GlobalFlags", "global_flags"),
    ("Features", "features"),
    ("Events", "events"),
    ("Commands", "commands"),
];

/// `$COLUMNS`, else a safe default.
fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse().ok())
        .filter(|&c: &usize| c >= 40)
        .unwrap_or(120)
}

/// PowerShell-style list, then error table.
pub(crate) fn render_report(report: &Value, width: usize) -> String {
    let pad = LIST_FIELDS.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
    let mut out = String::from("\n");
    for (label, key) in LIST_FIELDS {
        let value = match &report[*key] {
            Value::Object(map) => braces(map.keys().map(String::as_str)),
            Value::Array(items) => braces(items.iter().map(|v| v.as_str().unwrap_or_default())),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        out.push_str(&wrap_field(&format!("{label:<pad$} : "), &value, width));
    }
    out.push_str("\n");
    out.push_str(&error_table(report["errors"].as_array().map_or(&[][..], Vec::as_slice)));
    out.push_str("\n");
    out
}

/// `{a, b, c}` like PowerShell.
fn braces<'a>(items: impl Iterator<Item = &'a str>) -> String {
    format!("{{{}}}", items.collect::<Vec<_>>().join(", "))
}

/// Wrap at commas under the value.
fn wrap_field(prefix: &str, value: &str, width: usize) -> String {
    let indent = " ".repeat(prefix.len());
    let room = width.saturating_sub(prefix.len()).max(20);
    let mut out = String::new();
    let mut line = String::new();
    for word in value.split_inclusive(", ") {
        if !line.is_empty() && line.len() + word.trim_end().len() > room {
            out.push_str(if out.is_empty() { prefix } else { &indent });
            out.push_str(line.trim_end());
            out.push('\n');
            line.clear();
        }
        line.push_str(word);
    }
    out.push_str(if out.is_empty() { prefix } else { &indent });
    out.push_str(line.trim_end());
    out.push('\n');
    out
}

/// Errors as a `Format-Table`.
fn error_table(errors: &[Value]) -> String {
    let rows: Vec<(&str, String, &str)> = errors
        .iter()
        .map(|e| {
            (
                e["type"].as_str().unwrap_or_default(),
                e["exit_code"].to_string(),
                e["hint"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    let tw = rows.iter().map(|r| r.0.len()).max().unwrap_or(0).max("Type".len());
    let cw = "ExitCode".len();
    let mut out = format!("{:<tw$} {:>cw$} Hint\n", "Type", "ExitCode");
    out.push_str(&format!("{:<tw$} {:>cw$} ----\n", "----", "--------"));
    for (ty, code, hint) in rows {
        out.push_str(format!("{ty:<tw$} {code:>cw$} {hint}").trim_end());
        out.push('\n');
    }
    out
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
