use super::super::dry_run_json;
use aurelia::steam_client::{InstallPreview, InstallSizeEstimate};

#[test]
fn dry_run_json_adds_language_fields() {
    let preview = InstallPreview {
        estimate: InstallSizeEstimate {
            download_size: 5,
            disk_size: 9,
            depot_count: 2,
        },
        languages: vec!["english".into(), "german".into()],
        default_language: Some("german".into()),
        installed_language: None,
    };
    let value = dry_run_json(70, "windows", &preview);
    assert_eq!(
        value,
        serde_json::json!({
            "app_id": 70,
            "platform": "windows",
            "download_size": 5,
            "disk_size": 9,
            "depot_count": 2,
            "languages": ["english", "german"],
            "default_language": "german",
            "installed_language": null,
        })
    );
}

#[test]
fn language_flag_parses() {
    use clap::Parser;
    let cli =
        crate::app::cli::Cli::try_parse_from(["aurelia", "install", "70", "--language", "German"])
            .unwrap();
    let crate::app::cli::Command::Install(args) = cli.command else {
        panic!("not install");
    };
    assert_eq!(args.language.as_deref(), Some("German"));
}
