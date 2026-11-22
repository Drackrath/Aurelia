use super::super::*;
use crate::library::depot_browser::{ManifestRef, pick_manifest};

fn appinfo(text: &str) -> Vdf<'static> {
    steam_vdf_parser::parse_text(&format!(r#""appinfo" {{ {text} }}"#))
        .unwrap()
        .into_owned()
}

/// Among Us with Steamworks redist.
fn game() -> Vdf<'static> {
    appinfo(
        r#""depots" {
            "945361" { "name" "Among Us Content" "manifests" { "public" { "gid" "111" } } }
            "228990" {
                "name" "DirectX Jun 2010 Redist"
                "config" { "oslist" "windows" }
                "depotfromapp" "228980"
                "sharedinstall" "1"
            }
        }"#,
    )
}

fn redist() -> Vdf<'static> {
    appinfo(
        r#""depots" {
            "228990" {
                "name" "DirectX Jun 2010 Redist"
                "manifests" { "public" { "gid" "1829726630299308803" } "beta" { "gid" "42" } }
            }
        }"#,
    )
}

#[tokio::test]
async fn install_resolution_still_skips_sharedinstall() {
    let resolved = resolve_with(945360, game(), None, |_| {
        std::future::ready(Err(anyhow!("must not fetch")))
    })
    .await
    .unwrap();
    let infos = manifest_infos(&resolved.rows);
    assert!(infos.iter().all(|m| m.depot_id != 228990));
}

#[tokio::test]
async fn sharedinstall_depot_resolves_through_owner_app() {
    let mut fetched = Vec::new();
    let resolved = resolve_one_with(945360, game(), 228990, |app_id| {
        fetched.push(app_id);
        std::future::ready(Ok(redist()))
    })
    .await
    .unwrap();

    assert_eq!(fetched, vec![228980]);
    // Key and request code use this.
    assert_eq!(resolved.source_app(228990), 228980);
    let infos = manifest_infos(&resolved.rows);
    let public = pick_manifest(&infos, 228990, &ManifestRef::Branch("public".into())).unwrap();
    assert_eq!(public, 1829726630299308803);
    let beta = pick_manifest(&infos, 228990, &ManifestRef::Branch("beta".into())).unwrap();
    assert_eq!(beta, 42);
}

#[tokio::test]
async fn own_depot_resolves_without_fetching() {
    let resolved = resolve_one_with(945360, game(), 945361, |_| {
        std::future::ready(Err(anyhow!("must not fetch")))
    })
    .await
    .unwrap();
    assert_eq!(resolved.source_app(945361), 945360);
    let infos = manifest_infos(&resolved.rows);
    let id = pick_manifest(&infos, 945361, &ManifestRef::Branch("public".into())).unwrap();
    assert_eq!(id, 111);
}

#[tokio::test]
async fn unknown_depot_keeps_no_manifests_error() {
    let resolved = resolve_one_with(945360, game(), 5, |_| {
        std::future::ready(Err(anyhow!("must not fetch")))
    })
    .await
    .unwrap();
    let infos = manifest_infos(&resolved.rows);
    let err = pick_manifest(&infos, 5, &ManifestRef::Branch("public".into())).unwrap_err();
    assert!(err.to_string().contains("no manifests"));
}

#[tokio::test]
async fn owner_without_manifests_names_owner_app() {
    let empty = appinfo(r#""depots" { "228990" { "name" "Redist" } }"#);
    let err = resolve_one_with(945360, game(), 228990, |_| {
        std::future::ready(Ok(empty.clone()))
    })
    .await
    .err()
    .expect("missing owner manifests must fail");
    assert!(err.to_string().contains("app 228980"), "{err:#}");
}
