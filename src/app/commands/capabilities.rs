//! `capabilities`: what this build supports.

use crate::app::output::table::Table;
use anyhow::Result;
use clap::CommandFactory;
use serde_json::{Map, Value, json};

use aurelia::core::error::{ErrorKind, TypedError};

use crate::app::cli::Cli;
use crate::app::commands::common::*;

/// Every `event` name in JSON output.
pub(crate) const EVENTS: &[&str] = &[
    "awaiting_confirmation",
    "closed",
    "confirmation_methods",
    "echo",
    "error",
    "game_invite",
    "guard_required",
    "message",
    "not_found",
    "openid_challenge",
    "other",
    "progress",
    "pausing",
    "platform_install",
    "qr_challenge",
    "qr_scanned",
    "ready",
    "result",
    "shaders",
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
    // Dry run lists languages; `--language`.
    "install.languages",
    // Vulkan pipelines replayed before launch.
    "play.shader_precache",
    // `result` lines carry `schema`.
    "result.schema",
    // `config game --platform` is enforced.
    "play.pinned_platform",
];

pub(crate) fn cmd_capabilities(path: &[String], json: bool) -> Result<()> {
    if !path.is_empty() {
        let detail = command_detail(path)?;
        if json {
            print_json_line(&detail);
        } else {
            cli_print!(
                "{}",
                render_detail(&detail, crate::app::output::terminal_width())
            );
        }
        return Ok(());
    }
    let report = capabilities();
    if json {
        print_json_line(&report);
    } else {
        cli_print!(
            "{}",
            render_report(&report, crate::app::output::terminal_width())
        );
    }
    Ok(())
}

/// One command, flags in full.
pub(crate) fn command_detail(path: &[String]) -> Result<Value> {
    let root = Cli::command();
    let mut cmd = &root;
    for (depth, name) in path.iter().enumerate() {
        let visible: Vec<&clap::Command> =
            cmd.get_subcommands().filter(|c| !c.is_hide_set()).collect();
        let Some(next) = visible
            .iter()
            .find(|c| c.get_name() == name || c.get_all_aliases().any(|a| a == name))
        else {
            let known: Vec<&str> = visible.iter().map(|c| c.get_name()).collect();
            let at = if depth == 0 {
                "aurelia".to_string()
            } else {
                path[..depth].join(" ")
            };
            let hint = if known.is_empty() {
                format!("`{at}` has no subcommands")
            } else {
                format!("`{at}` has: {}", known.join(", "))
            };
            return Err(TypedError::new(
                ErrorKind::NotFound,
                format!("no command `{}`; {hint}", path[..=depth].join(" ")),
            )
            .into());
        };
        cmd = *next;
    }
    let args: Vec<Value> = cmd
        .get_positionals()
        .filter(|a| !a.is_hide_set())
        .map(|a| {
            json!({
                "name": a.get_id().as_str(),
                "required": a.is_required_set(),
                "multiple": a.get_num_args().is_some_and(|n| n.max_values() > 1),
                "help": a.get_help().map(ToString::to_string),
            })
        })
        .collect();
    let flags: Vec<Value> = cmd
        .get_arguments()
        .filter(|a| !a.is_hide_set() && !a.is_positional())
        .filter_map(|a| {
            let long = a.get_long()?;
            let takes_value = a.get_action().takes_values();
            // Switches report bool values; skip.
            let values: Vec<String> = if takes_value {
                a.get_possible_values()
                    .iter()
                    .filter(|v| !v.is_hide_set())
                    .map(|v| v.get_name().to_string())
                    .collect()
            } else {
                Vec::new()
            };
            let value_name = a
                .get_value_names()
                .and_then(|n| n.first())
                .map(|n| n.to_string())
                .unwrap_or_else(|| a.get_id().as_str().to_uppercase());
            Some(json!({
                "long": format!("--{long}"),
                "short": a.get_short().map(|c| format!("-{c}")),
                "value": takes_value.then_some(value_name),
                "values": values,
                "required": a.is_required_set(),
                "help": a.get_help().map(ToString::to_string),
            }))
        })
        .collect();
    let subcommands: Map<String, Value> = cmd
        .get_subcommands()
        .filter(|c| !c.is_hide_set())
        .map(|c| {
            (
                c.get_name().to_string(),
                c.get_about().map(ToString::to_string).into(),
            )
        })
        .collect();
    Ok(json!({
        "command": path.join(" "),
        "about": cmd.get_about().map(ToString::to_string),
        "args": args,
        "flags": flags,
        "subcommands": subcommands,
        "schema": JSON_SCHEMA,
    }))
}

/// Detail list, then flag table.
pub(crate) fn render_detail(detail: &Value, width: usize) -> String {
    let names = |key: &str| match &detail[key] {
        Value::Object(map) => braces(map.keys().map(String::as_str)),
        Value::Array(items) => braces(items.iter().map(|a| a["name"].as_str().unwrap_or_default())),
        _ => String::new(),
    };
    let fields = [
        (
            "Command",
            detail["command"].as_str().unwrap_or_default().to_string(),
        ),
        (
            "About",
            detail["about"].as_str().unwrap_or_default().to_string(),
        ),
        ("Args", names("args")),
        ("Subcommands", names("subcommands")),
    ];
    let pad = fields.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
    let mut out = String::from("\n");
    for (label, value) in &fields {
        out.push_str(&wrap_field(&format!("{label:<pad$} : "), value, width));
    }
    let flags = detail["flags"].as_array().map_or(&[][..], Vec::as_slice);
    if !flags.is_empty() {
        let rows: Vec<[String; 4]> = flags
            .iter()
            .map(|f| {
                let text = |k: &str| f[k].as_str().unwrap_or_default().to_string();
                let values = f["values"].as_array().filter(|v| !v.is_empty());
                let value = match values {
                    Some(v) => v
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("|"),
                    None => text("value"),
                };
                [text("long"), text("short"), value, text("help")]
            })
            .collect();
        out.push('\n');
        out.push_str(&table(&["Flag", "Short", "Value", "Help"], &rows, width));
    }
    out.push('\n');
    out
}

/// Flags as a `Format-Table`.
fn table(head: &[&str; 4], rows: &[[String; 4]], width: usize) -> String {
    let mut t = Table::new()
        .left(head[0])
        .left(head[1])
        .left(head[2])
        .left(head[3]);
    for r in rows {
        t.row(r.iter());
    }
    t.render(width)
}

/// Labels for the list view.
const LIST_FIELDS: &[(&str, &str)] = &[
    ("Name", "name"),
    ("Version", "version"),
    ("Codename", "codename"),
    ("Schema", "schema"),
    ("OS", "os"),
    ("Arch", "arch"),
    ("GlobalFlags", "global_flags"),
    ("Features", "features"),
    ("Events", "events"),
    ("Commands", "commands"),
];

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
    out.push('\n');
    out.push_str(&error_table(
        report["errors"].as_array().map_or(&[][..], Vec::as_slice),
        width,
    ));
    out.push('\n');
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
fn error_table(errors: &[Value], width: usize) -> String {
    let mut t = Table::new().left("Type").right("ExitCode").left("Hint");
    for e in errors {
        t.row([
            e["type"].as_str().unwrap_or_default().to_string(),
            e["exit_code"].to_string(),
            e["hint"].as_str().unwrap_or_default().to_string(),
        ]);
    }
    t.render(width)
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
        "version": aurelia::core::version::NUMBER,
        "codename": aurelia::core::version::CODENAME,
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
mod tests;
