use super::*;

#[test]
fn typed_error_survives_context_layers() {
    let err = anyhow::Error::new(
        TypedError::new(ErrorKind::RateLimited, "slow down")
            .with_retry_after(Some(Duration::from_secs(30))),
    )
    .context("outer");
    let c = classify(&err);
    assert_eq!(c.kind, ErrorKind::RateLimited);
    assert_eq!(c.retry_after, Some(Duration::from_secs(30)));
}

#[test]
fn login_rate_limit_is_rate_limited() {
    let err = anyhow::Error::new(LoginError::RateLimited).context("refresh token login failed");
    assert_eq!(classify(&err).kind, ErrorKind::RateLimited);
}

#[test]
fn invalid_credentials_is_auth_required() {
    let err = anyhow::Error::new(LoginError::InvalidCredentials);
    assert_eq!(classify(&err).kind, ErrorKind::AuthRequired);
}

#[test]
fn api_eresult_maps_through_network_error() {
    let err = anyhow::Error::new(NetworkError::ApiError(EResult::RateLimitExceeded));
    assert_eq!(classify(&err).kind, ErrorKind::RateLimited);
    let err = anyhow::Error::new(NetworkError::ApiError(EResult::AccessDenied));
    assert_eq!(classify(&err).kind, ErrorKind::AccessDenied);
    let err = anyhow::Error::new(NetworkError::Timeout);
    assert_eq!(classify(&err).kind, ErrorKind::NetworkTimeout);
}

#[test]
fn plain_errors_are_unknown() {
    let err = anyhow::anyhow!("something else");
    assert_eq!(classify(&err).kind, ErrorKind::Unknown);
    assert_eq!(ErrorKind::Unknown.exit_code(), 1);
    assert_eq!(ErrorKind::RateLimited.exit_code(), 75);
    assert_eq!(ErrorKind::AuthRequired.exit_code(), 77);
}

#[test]
fn kind_serializes_snake_case() {
    assert_eq!(
        serde_json::to_string(&ErrorKind::PrivacyRestricted).unwrap(),
        "\"privacy_restricted\""
    );
}

// Adding a variant breaks this match.
fn ordinal(kind: ErrorKind) -> usize {
    match kind {
        ErrorKind::InvalidInput => 0,
        ErrorKind::NotFound => 1,
        ErrorKind::RateLimited => 2,
        ErrorKind::AccessDenied => 3,
        ErrorKind::PrivacyRestricted => 4,
        ErrorKind::NetworkTimeout => 5,
        ErrorKind::SourceChanged => 6,
        ErrorKind::AuthRequired => 7,
        ErrorKind::LoginDenied => 8,
        ErrorKind::LoginExpired => 9,
        ErrorKind::Timeout => 10,
        ErrorKind::Cancelled => 11,
        ErrorKind::InputClosed => 12,
        ErrorKind::Unknown => 13,
    }
}

#[test]
fn all_lists_every_kind_once() {
    let mut seen: Vec<usize> = ErrorKind::ALL.iter().map(|k| ordinal(*k)).collect();
    seen.sort_unstable();
    assert_eq!(seen, (0..=13).collect::<Vec<_>>());
}
