use super::super::*;
use crate::steam_client::language::recorded_language;

fn appinfo(text: &str) -> Vdf<'static> {
    steam_vdf_parser::parse_text(&format!(r#""appinfo" {{ {text} }}"#))
        .unwrap()
        .into_owned()
}

#[test]
fn key_failures_skip_only_automatic_optional_or_denied_depots() {
    for eresult in [2, 5, 9, 15, 16, 20] {
        let error = anyhow::Error::new(DepotKeyError {
            depot_id: 476604,
            eresult,
        })
        .context("request failed");
        // Required: only access denied skips.
        assert_eq!(skip_key_failure(&error, false, true), eresult == 15);
        // Optional: any Steam refusal skips.
        assert!(skip_key_failure(&error, false, false));
        assert!(!skip_key_failure(&error, true, false));
    }
    // Transport errors never skip.
    assert!(!skip_key_failure(&anyhow!("network timeout"), false, false));
    assert!(!skip_key_failure(&anyhow!("eresult 15"), false, true));
}

#[test]
fn accessible_total_drops_skipped_depot_sizes() {
    let sizes: HashMap<u32, u64> = [(750921, 34_000), (849165, 2_000), (849166, 1_500)]
        .into_iter()
        .collect();
    assert_eq!(accessible_total(37_500, &sizes, &[]), 37_500);
    assert_eq!(accessible_total(37_500, &sizes, &[849165, 849166]), 34_000);
    // Depots without a known size leave the total alone.
    assert_eq!(accessible_total(37_500, &sizes, &[999_999]), 37_500);
    // Never underflows.
    assert_eq!(accessible_total(1_000, &sizes, &[750921]), 0);
}

#[test]
fn installed_dlc_depots_become_optional() {
    let mut selections = vec![
        ManifestSelection {
            app_id: 977950,
            depot_id: 977954,
            manifest_id: 1,
            appinfo_vdf: String::new(),
            required: true,
        },
        ManifestSelection {
            app_id: 977950,
            depot_id: 1977574,
            manifest_id: 7,
            appinfo_vdf: String::new(),
            required: true,
        },
    ];
    let dlc: HashSet<u64> = [1977574].into_iter().collect();
    mark_dlc_optional(&mut selections, &dlc);
    assert!(selections[0].required);
    assert!(!selections[1].required);
}

#[test]
fn denied_base_content_cannot_publish_empty_or_dlc_only_install() {
    assert!(!has_installable_content(0, true, 0));
    assert!(!has_installable_content(0, false, 0));
    assert!(!has_installable_content(2, true, 0));
    assert!(has_installable_content(2, true, 1));
    assert!(has_installable_content(1, false, 0));
}

#[test]
fn required_depot_distinguishes_base_game_and_dlc() {
    for (field, required) in [
        ("", true),
        (r#""dlcappid" "0""#, true),
        (r#""dlcappid" "10""#, true),
        (r#""dlcappid" "20""#, false),
    ] {
        let vdf = appinfo(&format!(r#""depot" {{ {field} }}"#));
        assert_eq!(
            required_depot(vdf.value().get("depot").unwrap(), 10),
            required
        );
    }
}

#[tokio::test]
async fn shared_depot_inherits_metadata_and_tracks_source_app() {
    let child = appinfo(
        r#""depots" {
            "100" { "depotfromapp" "20" "name" "Child name" }
            "101" { "depotfromapp" "20" }
        }"#,
    );
    let parent = appinfo(
        r#""depots" {
            "100" { "name" "Parent name" "manifests" { "public" { "gid" "123" } } }
            "101" { "manifests" { "public" { "gid" "456" } } }
        }"#,
    );
    let mut fetched = Vec::new();
    let resolved = resolve_with(10, child, None, |app_id| {
        fetched.push(app_id);
        std::future::ready(Ok(parent.clone()))
    })
    .await
    .unwrap();

    assert_eq!(fetched, vec![20]);
    assert_eq!(resolved.source_app(100), 20);
    assert_eq!(resolved.source_app(101), 20);
    assert_eq!(resolved.source_app(999), 10);
    let row = &resolved.rows["100"];
    assert_eq!(row.get("name").and_then(Value::as_str), Some("Child name"));
    assert_eq!(row.get_str(&["manifests", "public", "gid"]), Some("123"));
}

#[tokio::test]
async fn proxy_manifests_do_not_fetch_parent_appinfo() {
    let child = appinfo(
        r#""depots" {
            "100" { "depotfromapp" "20" "manifests" { "public" { "gid" "123" } } }
        }"#,
    );
    let resolved = resolve_with(10, child, None, |_| {
        std::future::ready(Err(anyhow!(
            "proxy manifests should not fetch parent appinfo"
        )))
    })
    .await
    .unwrap();

    assert_eq!(resolved.source_app(100), 20);
    assert_eq!(
        resolved.rows["100"].get_str(&["manifests", "public", "gid"]),
        Some("123"),
    );
}

#[tokio::test]
async fn required_shared_depot_cycle_is_rejected() {
    let child = appinfo(r#""depots" { "100" { "depotfromapp" "20" } }"#);
    let parent = appinfo(r#""depots" { "100" { "depotfromapp" "10" } }"#);
    let result = resolve_with(10, child, None, |_| std::future::ready(Ok(parent.clone()))).await;

    let error = result.err().expect("required depot cycle must fail");
    assert!(error.to_string().contains("cycle or depth limit"));
}

#[tokio::test]
async fn unavailable_optional_dlc_keeps_original_metadata() {
    let child = appinfo(r#""depots" { "100" { "depotfromapp" "20" "dlcappid" "30" } }"#);
    let resolved = resolve_with(10, child, None, |_| {
        std::future::ready(Err(anyhow!("fake appinfo unavailable")))
    })
    .await
    .unwrap();

    assert_eq!(resolved.source_app(100), 10);
    assert_eq!(
        resolved.rows["100"]
            .get("depotfromapp")
            .and_then(Value::as_str),
        Some("20"),
    );
}

fn selection_details(selections: Vec<ManifestSelection>) -> Vec<(u32, u32, u64, bool)> {
    let mut rows: Vec<_> = selections
        .into_iter()
        .map(|selection| {
            (
                selection.depot_id,
                selection.app_id,
                selection.manifest_id,
                selection.required,
            )
        })
        .collect();
    rows.sort_unstable();
    rows
}

#[tokio::test]
async fn update_and_verify_keep_shared_source_apps_and_dlc_classification() {
    let child = appinfo(
        r#""depots" {
            "100" { "depotfromapp" "20" }
            "101" { "depotfromapp" "20" "dlcappid" "40" }
        }"#,
    );
    let parent = appinfo(
        r#""depots" {
            "100" { "depotfromapp" "30" }
            "101" { "manifests" { "public" { "gid" "456" } } }
        }"#,
    );
    let grandparent = appinfo(
        r#""depots" {
            "100" { "manifests" { "public" { "gid" "123" } } }
        }"#,
    );
    let installed = HashMap::from([(100, 11), (101, 22)]);
    let resolved = resolve_with(10, child, Some(&installed), |app_id| {
        std::future::ready(match app_id {
            20 => Ok(parent.clone()),
            30 => Ok(grandparent.clone()),
            _ => Err(anyhow!("unexpected source app {app_id}")),
        })
    })
    .await
    .unwrap();

    assert_eq!(
        selection_details(resolved.download_selections(&installed, "public", false)),
        vec![(100, 30, 123, true), (101, 20, 456, false)],
    );
    assert_eq!(
        selection_details(resolved.download_selections(&installed, "public", true)),
        vec![(100, 30, 11, true), (101, 20, 22, false)],
    );
}

#[tokio::test]
async fn verify_keeps_pinned_manifests_without_current_branch_or_depot_metadata() {
    let child = appinfo(
        r#""depots" {
            "100" { "depotfromapp" "20" "manifests" { "beta" { "gid" "123" } } }
            "101" { "name" "Depot without current manifests" }
        }"#,
    );
    let installed = HashMap::from([(100, 11), (101, 22), (999, 33)]);
    let resolved = resolve_with(10, child, Some(&installed), |_| {
        std::future::ready(Err(anyhow!("proxy manifests need no parent request")))
    })
    .await
    .unwrap();

    assert_eq!(
        selection_details(resolved.download_selections(&installed, "public", true)),
        vec![
            (100, 20, 11, true),
            (101, 10, 22, true),
            (999, 10, 33, true)
        ],
    );
}

#[tokio::test]
async fn verify_resolves_shared_source_after_current_manifests_disappear() {
    let child = appinfo(r#""depots" { "100" { "depotfromapp" "20" } }"#);
    let parent = appinfo(r#""depots" { "100" { "name" "Archived depot" } }"#);
    let installed = HashMap::from([(100, 11)]);
    let resolved = resolve_with(10, child.clone(), Some(&installed), |_| {
        std::future::ready(Ok(parent.clone()))
    })
    .await
    .unwrap();

    assert_eq!(
        selection_details(resolved.download_selections(&installed, "public", true)),
        vec![(100, 20, 11, true)],
    );
    assert!(
        resolved
            .download_selections(&installed, "public", false)
            .is_empty()
    );

    let install = resolve_with(10, child, None, |_| std::future::ready(Ok(parent.clone()))).await;
    let error = install
        .err()
        .expect("fresh installs need current manifests");
    assert!(error.to_string().contains("has no manifests"));
}

#[tokio::test]
async fn update_uses_active_branch_and_public_fallback_for_installed_depots() {
    let child = appinfo(
        r#""depots" {
            "100" { "manifests" { "public" { "gid" "1000" } "beta" { "gid" "2000" } } }
            "101" { "manifests" { "public" { "gid" "1001" } } }
            "102" { "manifests" { "beta" { "gid" "2002" } } }
        }"#,
    );
    let installed = HashMap::from([(100, 11), (101, 22), (999, 33)]);
    let resolved = resolve_with(10, child, Some(&installed), |_| {
        std::future::ready(Err(anyhow!("ordinary depots need no parent request")))
    })
    .await
    .unwrap();

    assert_eq!(
        selection_details(resolved.download_selections(&installed, "beta", false)),
        vec![(100, 10, 2000, true), (101, 10, 1001, true)],
    );
    assert_eq!(
        selection_details(resolved.download_selections(&installed, "public", false)),
        vec![(100, 10, 1000, true), (101, 10, 1001, true)],
    );
}

#[tokio::test]
async fn update_without_installed_depots_keeps_all_advertised_manifests() {
    let child = appinfo(
        r#""depots" {
            "100" { "manifests" { "public" { "gid" "123" } } }
            "101" { "manifests" { "beta" { "gid" "456" } } }
        }"#,
    );
    let resolved = resolve_with(10, child, None, |_| {
        std::future::ready(Err(anyhow!("ordinary depots need no parent request")))
    })
    .await
    .unwrap();

    assert_eq!(
        selection_details(resolved.download_selections(&HashMap::new(), "beta", false)),
        vec![(100, 10, 123, true), (101, 10, 456, true)],
    );
    assert!(
        resolved
            .download_selections(&HashMap::new(), "beta", true)
            .is_empty()
    );
}

#[tokio::test]
async fn selected_resolution_ignores_uninstalled_broken_shared_references() {
    let child = appinfo(
        r#""depots" {
            "100" { "depotfromapp" "20" }
            "101" { "depotfromapp" "30" }
        }"#,
    );
    let parent = appinfo(
        r#""depots" {
            "100" { "manifests" { "public" { "gid" "123" } } }
        }"#,
    );
    let installed = HashMap::from([(100, 11)]);
    let mut fetched = Vec::new();
    let resolved = resolve_with(10, child, Some(&installed), |app_id| {
        fetched.push(app_id);
        std::future::ready(if app_id == 20 {
            Ok(parent.clone())
        } else {
            Err(anyhow!("uninstalled shared depot is unavailable"))
        })
    })
    .await
    .unwrap();

    assert_eq!(fetched, vec![20]);
    assert_eq!(
        selection_details(resolved.download_selections(&installed, "public", false)),
        vec![(100, 20, 123, true)],
    );
    assert_eq!(
        selection_details(resolved.download_selections(&installed, "public", true)),
        vec![(100, 20, 11, true)],
    );
}

#[tokio::test]
async fn selected_shared_dlc_metadata_failure_is_not_silently_skipped() {
    let child = appinfo(r#""depots" { "100" { "depotfromapp" "20" "dlcappid" "30" } }"#);
    let installed = HashMap::from([(100, 11)]);
    let result = resolve_with(10, child, Some(&installed), |_| {
        std::future::ready(Err(anyhow!("fake appinfo connection failure")))
    })
    .await;

    let error = result.err().expect("selected depot metadata must resolve");
    let message = format!("{error:#}");
    assert!(message.contains("shared depot 100 from app 20"));
    assert!(message.contains("fake appinfo connection failure"));
}

fn depot_rows(text: &str) -> Vdf<'static> {
    steam_vdf_parser::parse_text(&format!(r#""depots" {{ {text} }}"#))
        .unwrap()
        .into_owned()
}

/// Brotato-like: no language depots.
const NO_LANGUAGE_DEPOTS: &str = r#"
    "1942281" { "config" { "oslist" "windows" } }
    "1942282" { "config" { "oslist" "linux" } }
    "2868390" { "dlcappid" "2868390" }
"#;

/// BG3-like Linux language depots.
const LANGUAGE_DEPOTS: &str = r#"
    "2378501" { "config" { "oslist" "linux" } }
    "2378503" { "config" { "oslist" "linux" "language" "english" } }
    "2378505" { "config" { "oslist" "linux" "language" "german" } }
    "1086943" { "config" { "oslist" "windows" "language" "french" } }
"#;

/// Install via the real manifest writer.
fn fresh_install(dir: &Path, existing: Option<&str>, rows: &str, steam: &str) -> String {
    let path = dir.join("appmanifest_1.acf");
    if let Some(existing) = existing {
        std::fs::write(&path, existing).unwrap();
    }
    let manifest_language = existing.and_then(manifests::user_config_language);
    let wanted =
        crate::core::locale::pick_install_language(manifest_language.as_deref(), Some(steam), None);
    let vdf = depot_rows(rows);
    let recorded = recorded_language(
        vdf.as_obj().unwrap(),
        1,
        DepotPlatform::Linux,
        &wanted,
        false,
    );
    SteamClient::write_appmanifest(
        &path,
        AppManifestSpec {
            appid: 1,
            game_name: "Game",
            installdir: "Game",
            installed_depots: vec![(2378501, 1, 10)],
            buildid: Some("1"),
            fully_installed: true,
            pin_updates: false,
            language: recorded.as_deref(),
            platform_override: crate::steam_client::PlatformOverride::Keep,
        },
    )
    .unwrap();
    std::fs::read_to_string(&path).unwrap()
}

#[test]
fn no_language_depots_writes_no_language() {
    let vdf = depot_rows(NO_LANGUAGE_DEPOTS);
    let rows = vdf.as_obj().unwrap();
    assert_eq!(
        recorded_language(rows, 1, DepotPlatform::Linux, "german", false),
        None
    );
    assert_eq!(
        recorded_language(rows, 1, DepotPlatform::Linux, "english", false),
        None
    );

    let dir = tempfile::tempdir().unwrap();
    let raw = fresh_install(dir.path(), None, NO_LANGUAGE_DEPOTS, "german");
    assert_eq!(manifests::user_config_language(&raw), None);
}

#[test]
fn wanted_language_depot_is_recorded() {
    let vdf = depot_rows(LANGUAGE_DEPOTS);
    let rows = vdf.as_obj().unwrap();
    assert_eq!(
        recorded_language(rows, 1, DepotPlatform::Linux, "German", false).as_deref(),
        Some("german")
    );

    let dir = tempfile::tempdir().unwrap();
    let raw = fresh_install(dir.path(), None, LANGUAGE_DEPOTS, "german");
    assert_eq!(
        manifests::user_config_language(&raw).as_deref(),
        Some("german")
    );
}

#[test]
fn missing_language_depot_records_no_fallback() {
    let vdf = depot_rows(LANGUAGE_DEPOTS);
    let rows = vdf.as_obj().unwrap();
    // French exists for Windows only.
    assert_eq!(
        recorded_language(rows, 1, DepotPlatform::Linux, "french", false),
        None
    );
    assert_eq!(
        recorded_language(rows, 1, DepotPlatform::Windows, "french", false).as_deref(),
        Some("french")
    );
    assert_eq!(
        recorded_language(rows, 1, DepotPlatform::Linux, "polish", false),
        None
    );

    let dir = tempfile::tempdir().unwrap();
    let raw = fresh_install(dir.path(), None, LANGUAGE_DEPOTS, "polish");
    assert_eq!(manifests::user_config_language(&raw), None);
}

#[test]
fn existing_manifest_language_is_kept_verbatim() {
    let existing = "\"AppState\"\n{\n\t\"appid\"\t\t\"1\"\n\t\"StateFlags\"\t\t\"6\"\n\
        \t\"UserConfig\"\n\t{\n\t\t\"language\"\t\t\"polish\"\n\t\t\"DisabledDLC\"\t\t\"5\"\n\t}\n}\n";
    let block =
        "\"UserConfig\"\n\t{\n\t\t\"language\"\t\t\"polish\"\n\t\t\"DisabledDLC\"\t\t\"5\"\n\t}";
    let dir = tempfile::tempdir().unwrap();

    // No Polish depot: nothing recorded, kept.
    let raw = fresh_install(dir.path(), Some(existing), LANGUAGE_DEPOTS, "german");
    assert!(raw.contains(block));
    assert_eq!(raw.matches("\"language\"").count(), 1);

    // No language depots at all: kept.
    let raw = fresh_install(dir.path(), Some(existing), NO_LANGUAGE_DEPOTS, "german");
    assert!(raw.contains(block));
    assert_eq!(raw.matches("\"language\"").count(), 1);

    // Manifest language beats Steam's.
    let german = existing.replace("polish", "german");
    let raw = fresh_install(dir.path(), Some(&german), LANGUAGE_DEPOTS, "english");
    assert!(raw.contains(&block.replace("polish", "german")));
    assert_eq!(raw.matches("\"language\"").count(), 1);
}
