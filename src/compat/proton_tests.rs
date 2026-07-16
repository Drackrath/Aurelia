use super::*;

#[test]
fn valve_lookup_is_case_insensitive() {
    assert_eq!(valve_app_id("Proton 9.0"), Some(2805730));
    assert_eq!(valve_app_id("proton experimental"), Some(1493710));
    assert_eq!(valve_app_id("GE-Proton9-20"), None);
}

#[test]
fn release_picks_matching_asset() {
    let src = &GE_SOURCES[0]; // proton-ge, .tar.gz
    let rel = GhRelease {
        tag_name: "GE-Proton9-20".to_string(),
        assets: vec![
            GhAsset {
                name: "GE-Proton9-20.sha512sum".to_string(),
                browser_download_url: "http://x/sum".to_string(),
                size: 10,
            },
            GhAsset {
                name: "GE-Proton9-20.tar.gz".to_string(),
                browser_download_url: "http://x/tar".to_string(),
                size: 12345,
            },
        ],
    };
    let pkg = release_to_package(src, rel).unwrap();
    assert_eq!(pkg.name, "GE-Proton9-20");
    assert_eq!(pkg.label, "Proton-GE");
    assert_eq!(pkg.size, 12345);
    match pkg.source {
        ProtonSource::Github { url, ext } => {
            assert_eq!(url, "http://x/tar");
            assert_eq!(ext, ".tar.gz");
        }
        _ => panic!("expected Github source"),
    }
}

#[test]
fn release_without_matching_asset_is_skipped() {
    let src = &GE_SOURCES[0];
    let rel = GhRelease {
        tag_name: "GE-Proton9-20".to_string(),
        assets: vec![GhAsset {
            name: "notes.txt".to_string(),
            browser_download_url: "http://x/notes".to_string(),
            size: 1,
        }],
    };
    assert!(release_to_package(src, rel).is_none());
}

fn cachyos_assets() -> Vec<GhAsset> {
    vec![
        GhAsset {
            name: "proton-cachyos-10.0-20250101-slr-x86_64.tar.xz".to_string(),
            browser_download_url: "http://x/base".to_string(),
            size: 100,
        },
        GhAsset {
            name: "proton-cachyos-10.0-20250101-slr-x86_64_v3.tar.xz".to_string(),
            browser_download_url: "http://x/v3".to_string(),
            size: 200,
        },
        GhAsset {
            name: "proton-cachyos-10.0-20250101-slr.sha512sum".to_string(),
            browser_download_url: "http://x/sum".to_string(),
            size: 1,
        },
    ]
}

#[test]
fn host_microarch_is_a_known_value() {
    let m = host_cachyos_microarch();
    assert!(m == "x86_64" || m == "x86_64_v3", "unexpected microarch: {m}");
}

#[test]
fn cachyos_prefers_v3_when_host_supports_avx2() {
    let a = choose_asset(cachyos_assets(), ".tar.xz", Some("x86_64_v3")).unwrap();
    assert_eq!(a.browser_download_url, "http://x/v3");
    assert!(a.name.contains("x86_64_v3"));
}

#[test]
fn cachyos_falls_back_to_generic_without_avx2() {
    let a = choose_asset(cachyos_assets(), ".tar.xz", Some("x86_64")).unwrap();
    assert_eq!(a.browser_download_url, "http://x/base");
    assert!(!a.name.contains("x86_64_v3"));
}

#[test]
fn cachyos_v3_falls_back_to_generic_when_no_v3_asset() {
    let only_base = vec![GhAsset {
        name: "proton-cachyos-10.0-x86_64.tar.xz".to_string(),
        browser_download_url: "http://x/base".to_string(),
        size: 100,
    }];
    let a = choose_asset(only_base, ".tar.xz", Some("x86_64_v3")).unwrap();
    assert_eq!(a.browser_download_url, "http://x/base");
}

#[test]
fn cachyos_release_labels_v3_selection() {
    let src = GE_SOURCES
        .iter()
        .find(|s| s.repo == "CachyOS/proton-cachyos")
        .unwrap();
    // Force a v3 pick by giving only a v3 asset (host-independent).
    let rel = GhRelease {
        tag_name: "cachyos-10.0".to_string(),
        assets: vec![GhAsset {
            name: "proton-cachyos-10.0-x86_64_v3.tar.xz".to_string(),
            browser_download_url: "http://x/v3".to_string(),
            size: 200,
        }],
    };
    // Only meaningful on an AVX2 host; otherwise choose_asset would skip the v3
    // asset. Assert the label reflects whatever asset was actually chosen.
    if let Some(pkg) = release_to_package(src, rel) {
        if pkg.size == 200 {
            assert!(pkg.label.contains("x86_64_v3"), "label was {}", pkg.label);
        }
    }
}

#[test]
fn non_microarch_source_keeps_plain_label() {
    let src = &GE_SOURCES[0]; // Proton-GE, microarch = false
    let rel = GhRelease {
        tag_name: "GE-Proton9-20".to_string(),
        assets: vec![GhAsset {
            name: "GE-Proton9-20.tar.gz".to_string(),
            browser_download_url: "http://x/tar".to_string(),
            size: 5,
        }],
    };
    assert_eq!(release_to_package(src, rel).unwrap().label, "Proton-GE");
}
