use super::*;

fn appinfo(text: &str) -> Vdf<'static> {
    steam_vdf_parser::parse_text(&format!(r#""appinfo" {{ {text} }}"#))
        .unwrap()
        .into_owned()
}

#[test]
fn only_explicit_access_denial_is_skippable() {
    for eresult in [2, 5, 15, 16, 20] {
        let error = anyhow::Error::new(DepotKeyError {
            depot_id: 476604,
            eresult,
        })
        .context("request failed");
        assert_eq!(skip_denied_key(&error, false), eresult == 15);
        assert!(!skip_denied_key(&error, true));
    }
    assert!(!skip_denied_key(&anyhow!("network timeout"), false));
    assert!(!skip_denied_key(&anyhow!("eresult 15"), false));
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
    let resolved = resolve_with(10, child, |app_id| {
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
    let resolved = resolve_with(10, child, |_| {
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
    let result = resolve_with(10, child, |_| std::future::ready(Ok(parent.clone()))).await;

    let error = result.err().expect("required depot cycle must fail");
    assert!(error.to_string().contains("cycle or depth limit"));
}

#[tokio::test]
async fn unavailable_optional_dlc_keeps_original_metadata() {
    let child = appinfo(r#""depots" { "100" { "depotfromapp" "20" "dlcappid" "30" } }"#);
    let resolved = resolve_with(10, child, |_| {
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
