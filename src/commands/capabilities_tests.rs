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
