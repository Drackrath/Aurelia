use super::super::*;

#[test]
fn test_map_io_error_not_found() {
    let err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
    let mapped = map_io_error(&err, None);
    assert_eq!(mapped.kind, LaunchErrorKind::GameData);
    assert!(mapped.message.contains("not found"));
    assert_eq!(mapped.context.get("io_kind").unwrap(), "NotFound");
}

#[test]
fn test_map_io_error_permission_denied() {
    let err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
    let mapped = map_io_error(&err, None);
    assert_eq!(mapped.kind, LaunchErrorKind::Permission);
    assert!(mapped.message.contains("Access denied"));
}

#[test]
fn test_map_io_error_lock_detected() {
    // Simulating a "Resource busy" or "locked" error which often maps to 'Other' or 'WouldBlock' in std::io
    let err = std::io::Error::other("file is locked by another process");
    let mapped = map_io_error(&err, None);
    assert_eq!(mapped.kind, LaunchErrorKind::Process);
    assert!(mapped.message.contains("locked by another process"));
}

#[test]
fn test_map_io_error_with_explicit_dup_info() {
    let err = std::io::Error::other("generic error");
    let info = DuplicateInstanceInfo {
        detected: true,
        source: "lockfile".to_string(),
    };
    let mapped = map_io_error(&err, Some(&info));
    assert_eq!(mapped.kind, LaunchErrorKind::Process);
    assert!(
        mapped
            .message
            .contains("Ensure no other instance is running")
    );
    assert_eq!(
        mapped.context.get("duplicate_instance_detected").unwrap(),
        "true"
    );
    assert_eq!(
        mapped.context.get("duplicate_detection_source").unwrap(),
        "lockfile"
    );
}
