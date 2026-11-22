use super::super::*;
use crate::core::error::{ErrorKind, TypedError};

fn stops(outcome: &PoolError) -> bool {
    matches!(outcome, PoolError::Stop(_))
}

#[test]
fn disk_full_stops_with_typed_kind() {
    for err in [
        steam_cdn::Error::Io(std::io::Error::from(std::io::ErrorKind::StorageFull)),
        steam_cdn::Error::Unexpected("No space left on device (os error 28)".into()),
    ] {
        let PoolError::Stop(e) = download_failure("bin/game", &err) else {
            panic!("disk full must stop: {err}");
        };
        let typed = e.downcast_ref::<TypedError>().expect("typed error");
        assert_eq!(typed.kind, ErrorKind::DiskFull);
        assert!(e.to_string().contains("bin/game"));
    }
}

#[test]
fn other_local_io_errors_stop() {
    for kind in [
        std::io::ErrorKind::PermissionDenied,
        std::io::ErrorKind::ReadOnlyFilesystem,
        std::io::ErrorKind::NotFound,
    ] {
        let err = steam_cdn::Error::Io(std::io::Error::from(kind));
        assert!(stops(&download_failure("a", &err)), "{kind:?}");
    }
}

#[test]
fn network_errors_try_next_pool() {
    for err in [
        steam_cdn::Error::Request("error sending request: connection reset".into()),
        steam_cdn::Error::Network("chunk timed out".into()),
        steam_cdn::Error::HttpStatus(reqwest::StatusCode::NOT_FOUND),
        steam_cdn::Error::Decompress("bad lzma".into()),
    ] {
        assert!(
            matches!(download_failure("a", &err), PoolError::Next(_)),
            "{err}"
        );
    }
}
