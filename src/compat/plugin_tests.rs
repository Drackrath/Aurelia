use super::*;
use crate::compat::proton::GhAsset;

fn assets(names: &[&str]) -> Vec<GhAsset> {
    names
        .iter()
        .map(|name| GhAsset {
            name: name.to_string(),
            browser_download_url: format!("http://x/{name}"),
            size: 1,
        })
        .collect()
}

#[test]
fn umu_zipapp_plain_tar_is_picked() {
    // umu-launcher 1.4.4: distro packages plus one uncompressed zipapp tarball.
    let a = assets(&[
        "python3-umu-launcher_1.4.4-1_amd64_debian-12.deb",
        "umu-launcher-1.4.4-zipapp.tar",
        "umu-launcher-1.4.4.fc43.x86_64.rpm",
    ]);
    let (asset, ext) = pick_archive(&a).unwrap();
    assert_eq!(asset.name, "umu-launcher-1.4.4-zipapp.tar");
    assert_eq!(ext, ".tar");
}

#[test]
fn compressed_tarball_beats_plain_tar() {
    let a = assets(&["x.tar", "x.tar.xz", "x.tar.gz"]);
    assert_eq!(pick_archive(&a).unwrap().1, ".tar.gz");
    let a = assets(&["x.tar", "x.tar.xz"]);
    assert_eq!(pick_archive(&a).unwrap().1, ".tar.xz");
}

#[test]
fn checksums_and_packages_are_not_archives() {
    let a = assets(&["x.tar.gz.sha512sum", "x.deb", "x.rpm"]);
    assert!(pick_archive(&a).is_none());
}
