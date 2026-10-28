use super::*;

use std::collections::BTreeSet;
use std::path::Path;

// `"event": "<name>"` literals under src/.
fn source_events(dir: &Path, found: &mut BTreeSet<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            source_events(&path, found);
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy();
        if !name.ends_with(".rs") || name.ends_with("_tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for (at, _) in text.match_indices("\"event\":") {
            let rest = text[at + "\"event\":".len()..].trim_start();
            if let Some(quoted) = rest.strip_prefix('"') {
                if let Some(end) = quoted.find('"') {
                    found.insert(quoted[..end].to_string());
                }
            }
        }
    }
}

#[test]
fn events_match_the_source() {
    let mut found = BTreeSet::new();
    source_events(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut found);
    let listed: BTreeSet<String> = EVENTS.iter().map(|e| e.to_string()).collect();
    assert_eq!(listed, found, "update capabilities::EVENTS");
}

#[test]
fn report_has_the_contract_fields() {
    let r = capabilities();
    assert_eq!(r["name"], "aurelia");
    assert_eq!(r["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(r["schema"], JSON_SCHEMA);
    assert!(r["global_flags"].as_array().unwrap().contains(&"--json".into()));
    assert!(r["features"].as_array().unwrap().contains(&"login.qr.stdin_control".into()));
}

#[test]
fn commands_carry_flags_args_and_subcommands() {
    let r = capabilities();
    let login = &r["commands"]["login"];
    let flags = login["flags"].as_array().unwrap();
    assert!(flags.contains(&"--qr".into()));
    assert!(flags.contains(&"--timeout".into()));
    // Hidden flags stay out.
    assert!(!flags.contains(&"--openid".into()));
    assert_eq!(r["commands"]["play"]["args"], json!(["app_id"]));
    assert!(r["commands"]["install"]["subcommands"]["stop"].is_object());
    // Globals aren't repeated per command.
    assert!(!flags.contains(&"--json".into()));
}

#[test]
fn errors_list_types_and_exit_codes() {
    let r = capabilities();
    let errors = r["errors"].as_array().unwrap();
    assert_eq!(errors.len(), ErrorKind::ALL.len());
    let denied = errors.iter().find(|e| e["type"] == "login_denied").unwrap();
    assert_eq!(denied["exit_code"], 80);
    assert!(denied["hint"].is_string());
}

#[test]
fn list_view_is_padded_and_braced() {
    let text = render_report(&capabilities(), 120);
    let name = text.lines().find(|l| l.starts_with("Name ")).unwrap();
    assert_eq!(name, "Name        : aurelia");
    assert!(text.contains("GlobalFlags : {--json, --verbose}"));
    assert!(text.contains("Features    : {login.qr.result_line,"));
}

#[test]
fn long_values_wrap_under_the_value() {
    let text = render_report(&capabilities(), 60);
    let lines: Vec<&str> = text.lines().collect();
    let at = lines.iter().position(|l| l.starts_with("Commands")).unwrap();
    assert!(lines[at + 1].starts_with(&" ".repeat("Commands    : ".len())));
    let list = &lines[..lines.iter().position(|l| l.starts_with("Type ")).unwrap()];
    assert!(list.iter().all(|l| l.len() <= 60), "{text}");
}

#[test]
fn errors_render_as_a_table() {
    let text = render_report(&capabilities(), 120);
    let lines: Vec<&str> = text.lines().collect();
    let head = lines.iter().position(|l| l.starts_with("Type ")).unwrap();
    assert!(lines[head].ends_with("ExitCode Hint"));
    assert!(lines[head + 1].starts_with("---- ") && lines[head + 1].ends_with("-------- ----"));
    // Codes right-align under ExitCode.
    let code_end = lines[head].find("ExitCode").unwrap() + "ExitCode".len();
    let denied = lines.iter().find(|l| l.starts_with("login_denied ")).unwrap();
    assert_eq!(&denied[code_end - 2..code_end], "80");
    assert!(denied[code_end..].starts_with(" the sign-in was declined"));
    let unknown = lines.iter().find(|l| l.starts_with("unknown ")).unwrap();
    assert_eq!(unknown.len(), code_end, "no trailing hint padding");
}
