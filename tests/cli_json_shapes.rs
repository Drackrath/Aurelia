//! CLI guard: offline `--json` output shapes.

mod common;

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::{TempDir, tempdir};

struct Sandbox {
    _tmp: TempDir,
    home: PathBuf,
    config: PathBuf,
    steam: PathBuf,
}

const CACHE_V026: &str = r#"{
  "steam_id": 76561197960287930,
  "games": [
    { "app_id": 570, "name": "Dota 2", "playtime_forever_minutes": 42,
      "local_manifest_ids": {}, "update_available": false },
    { "app_id": 440, "name": "Team Fortress 2", "playtime_forever_minutes": 0,
      "local_manifest_ids": {}, "update_available": false }
  ]
}"#;

const CACHE_V025: &str = r#"[
  { "app_id": 570, "name": "Dota 2", "playtime_forever_minutes": 42,
    "local_manifest_ids": {}, "update_available": false },
  { "app_id": 440, "name": "Team Fortress 2", "playtime_forever_minutes": 0,
    "local_manifest_ids": {}, "update_available": false }
]"#;

fn sandbox(cache: &str) -> Sandbox {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    let config = tmp.path().join("config");
    let steam = home.join(".local/share/Steam");
    let steamapps = steam.join("steamapps");
    fs::create_dir_all(steamapps.join("common/dota 2 beta")).unwrap();
    fs::create_dir_all(&config).unwrap();
    fs::write(
        steamapps.join("common/dota 2 beta/dota2.exe"),
        b"MZ not a real exe",
    )
    .unwrap();
    fs::write(
        steamapps.join("libraryfolders.vdf"),
        format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t\t\"apps\"\n\t\t{{\n\t\t\t\"570\"\t\t\"1000\"\n\t\t}}\n\t}}\n}}\n",
            common::vdf_path(&steam)
        ),
    )
    .unwrap();
    fs::write(
        steamapps.join("appmanifest_570.acf"),
        "\"AppState\"\n{\n\t\"appid\"\t\t\"570\"\n\t\"name\"\t\t\"Dota 2\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"installdir\"\t\t\"dota 2 beta\"\n\t\"buildid\"\t\t\"1\"\n\t\"SizeOnDisk\"\t\t\"1000\"\n\t\"UserConfig\"\n\t{\n\t\t\"BetaKey\"\t\t\"public\"\n\t}\n\t\"InstalledDepots\"\n\t{\n\t\t\"573\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"123\"\n\t\t\t\"size\"\t\t\"1000\"\n\t\t}\n\t}\n}\n",
    )
    .unwrap();
    fs::write(
        config.join("config.json"),
        format!(
            r#"{{ "steam_library_path": {}, "proton_version": "x", "enable_cloud_sync": false }}"#,
            common::json_path(&steam)
        ),
    )
    .unwrap();
    fs::write(config.join("library_cache.json"), cache).unwrap();
    Sandbox {
        _tmp: tmp,
        home,
        config,
        steam,
    }
}

fn aurelia(sb: &Sandbox, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aurelia"));
    cmd.args(args)
        .arg("--json")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &sb.home)
        .env("XDG_CONFIG_HOME", sb.home.join(".config"))
        .env("XDG_DATA_HOME", sb.home.join(".local/share"))
        .env("XDG_CACHE_HOME", sb.home.join(".cache"))
        .env("AURELIA_CONFIG_DIR", &sb.config)
        .env("AURELIA_NO_DAEMON", "1")
        .env("AURELIA_DISABLE_KEYRING", "1");
    cmd.output().expect("failed running the aurelia binary")
}

/// Type skeleton: keys kept, values typed.
fn shape(v: &Value) -> Value {
    match v {
        Value::Null => Value::from("null"),
        Value::Bool(_) => Value::from("bool"),
        Value::Number(_) => Value::from("number"),
        Value::String(_) => Value::from("string"),
        Value::Array(items) => {
            // Distinct item shapes, sorted.
            let mut shapes: Vec<String> = items
                .iter()
                .map(|i| serde_json::to_string(&shape(i)).unwrap())
                .collect();
            shapes.sort();
            shapes.dedup();
            Value::Array(
                shapes
                    .iter()
                    .map(|s| serde_json::from_str(s).unwrap())
                    .collect(),
            )
        }
        Value::Object(map) => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), shape(v))).collect())
        }
    }
}

fn run(sb: &Sandbox, args: &[&str]) -> (i32, Value) {
    let out = aurelia(sb, args);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let text = if stdout.trim().is_empty() {
        &stderr
    } else {
        &stdout
    };
    let value = serde_json::from_str(text.trim())
        .unwrap_or_else(|e| panic!("{args:?}: not JSON ({e}): stdout={stdout} stderr={stderr}"));
    (out.status.code().unwrap_or(-1), value)
}

/// Fixture games only; host mounts leak.
fn fixture_games(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .filter(|g| g["app_id"] == 570 || g["app_id"] == 440)
                .collect(),
        ),
        other => other,
    }
}

fn entry(sb: &Sandbox, args: &[&str]) -> (String, Value) {
    let (code, value) = run(sb, args);
    let value = fixture_games(value);
    let line = format!(
        "{} => exit {code} {}\n",
        args.join(" "),
        serde_json::to_string(&shape(&value)).unwrap()
    );
    (line, value)
}

fn check_snapshot(actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/cli_json_shapes.txt");
    if std::env::var_os("AURELIA_UPDATE_SNAPSHOTS").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, actual).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path).expect("missing cli_json_shapes.txt snapshot");
    for (a, e) in actual.lines().zip(expected.lines()) {
        assert_eq!(a, e, "--json shape changed");
    }
    assert_eq!(actual.lines().count(), expected.lines().count());
}

#[test]
fn json_shapes_match_snapshot() {
    let sb = sandbox(CACHE_V026);
    let mut out = String::new();
    let commands: &[&[&str]] = &[
        &["collections", "create", "Favs"],
        &["collections", "add", "Favs", "570"],
        &["list"],
        &["list", "-i"],
        &["libraries"],
        &["config", "show"],
        &["config", "game", "570"],
        &["collections", "list"],
        &["collections", "show", "Favs"],
        &["scripts", "list"],
        &["launch-targets", "570"],
        &["launch-options", "570"],
        &["cloud", "sync", "570"],
        &["info", "570"],
        &["install", "570", "--dry-run"],
    ];
    for args in commands {
        out.push_str(&entry(&sb, args).0);
    }
    check_snapshot(&out);
}

#[test]
fn list_reports_installed_game_from_fake_steam_root() {
    let sb = sandbox(CACHE_V026);
    let (code, value) = run(&sb, &["list"]);
    assert_eq!(code, 0);
    let value = fixture_games(value);
    let games = value.as_array().unwrap();
    assert_eq!(games.len(), 2, "{value:#}");
    let dota = games.iter().find(|g| g["app_id"] == 570).unwrap();
    assert_eq!(dota["is_installed"], true);
    assert_eq!(dota["name"], "Dota 2");
    assert_eq!(
        Path::new(dota["install_path"].as_str().unwrap()),
        sb.steam.join("steamapps/common/dota 2 beta")
    );
    assert_eq!(dota["store_url"], "https://store.steampowered.com/app/570");
}

#[test]
fn v025_bare_array_cache_lists_the_same_games() {
    let new = sandbox(CACHE_V026);
    let old = sandbox(CACHE_V025);
    let names = |sb: &Sandbox| -> Vec<(Value, Value)> {
        let (code, value) = run(sb, &["list"]);
        assert_eq!(code, 0);
        fixture_games(value)
            .as_array()
            .unwrap()
            .iter()
            .map(|g| (g["app_id"].clone(), g["is_installed"].clone()))
            .collect()
    };
    assert_eq!(names(&old), names(&new));
}

#[test]
fn auth_required_error_is_typed() {
    let sb = sandbox(CACHE_V026);
    let (code, value) = run(&sb, &["launch-options", "570"]);
    assert_ne!(code, 0);
    assert_eq!(value["type"], "auth_required", "{value}");
}
