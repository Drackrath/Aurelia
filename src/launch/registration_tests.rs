//! Fixture tests for the in-Wine client library registration and the
//! anonymous-client login preflight (docs Q12).

use super::*;

/// Appmanifest with no `SizeOnDisk` but two installed depots (100 + 250 bytes).
const ORPHANED_ACF: &str = "\"AppState\"\n{\n\t\"appid\"\t\t\"620\"\n\t\"name\"\t\t\"Portal 2\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"installdir\"\t\t\"Portal 2\"\n\t\"InstalledDepots\"\n\t{\n\t\t\"620\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"123\"\n\t\t\t\"size\"\t\t\"100\"\n\t\t}\n\t\t\"621\"\n\t\t{\n\t\t\t\"manifest\"\t\t\"456\"\n\t\t\t\"size\"\t\t\"250\"\n\t\t}\n\t}\n}\n";

/// Appmanifest that already carries a SizeOnDisk.
const SIZED_ACF: &str = "\"AppState\"\n{\n\t\"appid\"\t\t\"440\"\n\t\"name\"\t\t\"Team Fortress 2\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"SizeOnDisk\"\t\t\"9000\"\n}\n";

fn touch(path: &std::path::Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "x").unwrap();
}

// --- anonymous-client detection matrix -------------------------------------

#[test]
fn anonymous_when_nothing_present() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(!master_client_logged_in(tmp.path()));
}

#[test]
fn anonymous_with_loginusers_but_no_sentry() {
    let tmp = tempfile::tempdir().unwrap();
    touch(&tmp.path().join("config/loginusers.vdf"));
    assert!(!master_client_logged_in(tmp.path()));
}

#[test]
fn anonymous_with_sentry_but_no_loginusers() {
    let tmp = tempfile::tempdir().unwrap();
    touch(&tmp.path().join("ssfn1234567890123456789"));
    assert!(!master_client_logged_in(tmp.path()));
}

#[test]
fn logged_in_with_loginusers_and_sentry() {
    let tmp = tempfile::tempdir().unwrap();
    touch(&tmp.path().join("config/loginusers.vdf"));
    touch(&tmp.path().join("ssfn1234567890123456789"));
    assert!(master_client_logged_in(tmp.path()));
}

#[test]
fn ssfn_directory_does_not_count_as_sentry() {
    let tmp = tempfile::tempdir().unwrap();
    touch(&tmp.path().join("config/loginusers.vdf"));
    std::fs::create_dir_all(tmp.path().join("ssfn_dir")).unwrap();
    assert!(!master_client_logged_in(tmp.path()));
}

// --- apps-map collection + SizeOnDisk synthesis ----------------------------

#[test]
fn collect_native_apps_synthesizes_and_repairs_size_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let steamapps = tmp.path().join("steamapps");
    std::fs::create_dir_all(&steamapps).unwrap();
    std::fs::write(steamapps.join("appmanifest_620.acf"), ORPHANED_ACF).unwrap();
    std::fs::write(steamapps.join("appmanifest_440.acf"), SIZED_ACF).unwrap();
    // Non-manifest files are ignored.
    std::fs::write(steamapps.join("libraryfolders.vdf"), "\"libraryfolders\"\n{\n}\n").unwrap();

    let apps = collect_native_apps(&steamapps);
    assert_eq!(apps, vec![(440, 9000), (620, 350)]);

    // The orphaned ACF was repaired in place with the synthesized size.
    let repaired = std::fs::read_to_string(steamapps.join("appmanifest_620.acf")).unwrap();
    assert!(repaired.contains("\"SizeOnDisk\""));
    assert!(repaired.contains("\"350\""));
    // The already-sized ACF was left untouched.
    assert_eq!(std::fs::read_to_string(steamapps.join("appmanifest_440.acf")).unwrap(), SIZED_ACF);
}

// --- registration write ----------------------------------------------------

#[test]
fn registration_writes_wine_path_and_apps_to_both_vdfs() {
    let tmp = tempfile::tempdir().unwrap();
    let steam_dir = tmp.path().join("Steam");
    std::fs::create_dir_all(&steam_dir).unwrap();

    let native_root = std::path::Path::new("/home/user/SteamLibrary");
    let wine_path = crate::library::relocate::to_wine_path(native_root);
    assert_eq!(wine_path, "Z:\\home\\user\\SteamLibrary");

    let written =
        write_library_registration(&steam_dir, &wine_path, &[(620, 350), (440, 9000)]).unwrap();
    assert_eq!(written.len(), 2);

    for rel in ["config/libraryfolders.vdf", "steamapps/libraryfolders.vdf"] {
        let text = std::fs::read_to_string(steam_dir.join(rel)).unwrap();
        // Z:\ path lands VDF-escaped (doubled backslashes).
        assert!(
            text.contains("\"Z:\\\\home\\\\user\\\\SteamLibrary\""),
            "{rel} missing escaped wine path:\n{text}"
        );
        assert!(text.contains("\"620\"\t\t\"350\""), "{rel} missing apps entry");
        assert!(text.contains("\"440\"\t\t\"9000\""), "{rel} missing apps entry");
        // Seeded template keeps the client's own C: entry.
        assert!(text.contains("C:\\\\Program Files (x86)\\\\Steam"));
    }

    // Registration is now detectable, and idempotent (no duplicate entries).
    assert!(master_library_registered(&steam_dir, native_root));
    write_library_registration(&steam_dir, &wine_path, &[(620, 350)]).unwrap();
    let text = std::fs::read_to_string(steam_dir.join("config/libraryfolders.vdf")).unwrap();
    assert_eq!(text.matches("SteamLibrary").count(), 1, "entry must be replaced, not duplicated");
}

#[test]
fn registration_survives_malformed_existing_vdf() {
    let tmp = tempfile::tempdir().unwrap();
    let steam_dir = tmp.path().join("Steam");
    std::fs::create_dir_all(steam_dir.join("config")).unwrap();
    // A truncated/corrupt file with no closing brace at all.
    std::fs::write(steam_dir.join("config/libraryfolders.vdf"), "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\bro").unwrap();

    let native_root = std::path::Path::new("/home/user/SteamLibrary");
    let wine_path = crate::library::relocate::to_wine_path(native_root);
    write_library_registration(&steam_dir, &wine_path, &[(620, 350)]).unwrap();

    let text = std::fs::read_to_string(steam_dir.join("config/libraryfolders.vdf")).unwrap();
    assert!(text.starts_with("\"libraryfolders\""));
    assert!(text.contains("\"Z:\\\\home\\\\user\\\\SteamLibrary\""));
    assert!(text.contains("\"620\"\t\t\"350\""));
    assert!(master_library_registered(&steam_dir, native_root));
}

#[test]
fn not_registered_when_files_absent_or_other_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let steam_dir = tmp.path().join("Steam");
    std::fs::create_dir_all(&steam_dir).unwrap();
    let native_root = std::path::Path::new("/home/user/SteamLibrary");
    assert!(!master_library_registered(&steam_dir, native_root));

    std::fs::create_dir_all(steam_dir.join("config")).unwrap();
    std::fs::write(
        steam_dir.join("config/libraryfolders.vdf"),
        "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"Z:\\\\somewhere\\\\else\"\n\t}\n}\n",
    )
    .unwrap();
    assert!(!master_library_registered(&steam_dir, native_root));
}
