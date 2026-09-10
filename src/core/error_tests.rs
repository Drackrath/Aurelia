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
