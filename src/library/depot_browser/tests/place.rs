use super::super::*;

const ABC_SHA1: &str = "a9993e364706816aba3e25717850c26c9cd0d89d";

fn check(sha1: &str, force: bool) -> PlaceCheck<'_> {
    PlaceCheck {
        sha1,
        size: 3,
        force,
    }
}

#[test]
fn target_is_file_name_in_out_dir() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("new");
    let dest = download_target(&out, "bin/x64/game.exe").unwrap();
    assert_eq!(dest, out.canonicalize().unwrap().join("game.exe"));
    assert_eq!(
        part_path(&dest),
        out.canonicalize().unwrap().join("game.exe.part")
    );
}

#[test]
fn target_rejects_path_escape() {
    let dir = tempfile::tempdir().unwrap();
    assert!(download_target(dir.path(), "../.bashrc").is_err());
    assert!(download_target(dir.path(), "bin/../../x").is_err());
    assert!(download_target(dir.path(), "").is_err());
    assert!(download_target(dir.path(), "/").is_err());
}

#[cfg(unix)]
#[test]
fn target_rejects_symlink_leaving_out_dir() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path().join("x"), dir.path().join("x")).unwrap();
    assert!(download_target(dir.path(), "x").is_err());
}

#[test]
fn check_dest_needs_force_for_existing() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("f");
    assert!(check_dest(&dest, false).is_ok());
    std::fs::write(&dest, b"x").unwrap();
    let err = check_dest(&dest, false).unwrap_err().to_string();
    assert!(err.contains("--force"), "{err}");
    assert!(check_dest(&dest, true).is_ok());
    assert!(check_dest(dir.path(), true).is_err());
}

#[test]
fn create_part_replaces_stale_part() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("f");
    std::fs::write(part_path(&dest), b"stale").unwrap();
    let part = create_part(&dest).unwrap();
    assert_eq!(std::fs::read(&part).unwrap(), b"");
}

#[test]
fn place_sha_mismatch_removes_part() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("f");
    let part = part_path(&dest);
    std::fs::write(&part, b"abd").unwrap();
    let err = place(&part, &dest, check(ABC_SHA1, false)).unwrap_err();
    assert!(err.to_string().contains("checksum"), "{err}");
    assert!(!part.exists());
    assert!(!dest.exists());
}

#[test]
fn place_success_renames() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("f");
    let part = part_path(&dest);
    std::fs::write(&part, b"abc").unwrap();
    place(&part, &dest, check(ABC_SHA1, false)).unwrap();
    assert!(!part.exists());
    assert_eq!(std::fs::read(&dest).unwrap(), b"abc");
}

#[test]
fn place_keeps_existing_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("f");
    let part = part_path(&dest);
    std::fs::write(&dest, b"old").unwrap();
    std::fs::write(&part, b"abc").unwrap();
    assert!(place(&part, &dest, check(ABC_SHA1, false)).is_err());
    assert_eq!(std::fs::read(&dest).unwrap(), b"old");
    assert!(!part.exists());

    std::fs::write(&part, b"abc").unwrap();
    place(&part, &dest, check(&ABC_SHA1.to_uppercase(), true)).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b"abc");
}
