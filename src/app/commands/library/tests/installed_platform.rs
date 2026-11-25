use super::super::*;

fn platform(files: &[&str]) -> Option<String> {
    let dir = tempfile::tempdir().unwrap();
    for file in files {
        let path = dir.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }
    detect_installed_platform(dir.path().to_str().unwrap())
}

#[test]
fn app_bundle_is_macos() {
    let files = [
        "Balatro.app/Contents/Info.plist",
        "Balatro.app/Contents/MacOS/love",
    ];
    assert_eq!(platform(&files).as_deref(), Some("macos"));
}

#[test]
fn exe_inside_a_bundle_stays_macos() {
    let files = [
        "Game.app/Contents/Info.plist",
        "Game.app/Contents/MacOS/Game.exe",
    ];
    assert_eq!(platform(&files).as_deref(), Some("macos"));
}

#[test]
fn exe_is_windows_and_anything_else_linux() {
    assert_eq!(
        platform(&["Game.exe", "data/pak0"]).as_deref(),
        Some("windows")
    );
    assert_eq!(
        platform(&["game.x86_64", "Contents/Info.plist"]).as_deref(),
        Some("linux")
    );
}
