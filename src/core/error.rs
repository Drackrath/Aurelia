//! Stable error kinds for JSON output.

use std::fmt;
use std::time::Duration;

use steam_vent::{EResult, LoginError, NetworkError};

/// Machine-readable kind (`"type"` in JSON errors).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    InvalidInput,
    NotFound,
    RateLimited,
    AccessDenied,
    PrivacyRestricted,
    NetworkTimeout,
    SourceChanged,
    AuthRequired,
    Unknown,
}

impl ErrorKind {
    /// Driver guidance; `None` when message suffices.
    pub fn hint(self) -> Option<&'static str> {
        match self {
            Self::RateLimited => Some("Steam is throttling this client; pause before retrying"),
            Self::AuthRequired => Some("run `aurelia login`"),
            Self::PrivacyRestricted => Some("the profile or list is private or friends-only"),
            Self::NetworkTimeout => Some("check connectivity and retry"),
            Self::SourceChanged => Some("Steam changed a response shape; update Aurelia"),
            _ => None,
        }
    }

    /// Exit code; sysexits for scriptable kinds.
    pub fn exit_code(self) -> i32 {
        match self {
            Self::RateLimited => 75,
            Self::AuthRequired => 77,
            _ => 1,
        }
    }
}

/// Error with a kind known at source.
#[derive(Debug)]
pub struct TypedError {
    pub kind: ErrorKind,
    pub message: String,
    pub retry_after: Option<Duration>,
}

impl TypedError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn with_retry_after(mut self, retry_after: Option<Duration>) -> Self {
        self.retry_after = retry_after;
        self
    }
}

impl fmt::Display for TypedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some(d) = self.retry_after {
            write!(f, " (retry after {}s)", d.as_secs())?;
        }
        Ok(())
    }
}

impl std::error::Error for TypedError {}

/// Outcome of classifying an error chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Classified {
    pub kind: ErrorKind,
    pub retry_after: Option<Duration>,
}

/// Walk the cause chain, outermost first.
pub fn classify(err: &anyhow::Error) -> Classified {
    for cause in err.chain() {
        if let Some(t) = cause.downcast_ref::<TypedError>() {
            return Classified {
                kind: t.kind,
                retry_after: t.retry_after,
            };
        }
        if let Some(n) = cause.downcast_ref::<NetworkError>() {
            if let Some(kind) = network_kind(n) {
                return Classified {
                    kind,
                    retry_after: None,
                };
            }
        }
        if let Some(l) = cause.downcast_ref::<LoginError>() {
            if let Some(kind) = login_kind(l) {
                return Classified {
                    kind,
                    retry_after: None,
                };
            }
        }
        if let Some(r) = cause.downcast_ref::<reqwest::Error>() {
            if r.is_timeout() || r.is_connect() {
                return Classified {
                    kind: ErrorKind::NetworkTimeout,
                    retry_after: None,
                };
            }
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            if matches!(
                io.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::ConnectionRefused
            ) {
                return Classified {
                    kind: ErrorKind::NetworkTimeout,
                    retry_after: None,
                };
            }
        }
    }
    Classified {
        kind: ErrorKind::Unknown,
        retry_after: None,
    }
}

/// Kind for a Steam `EResult`, if any.
pub fn eresult_kind(result: EResult) -> Option<ErrorKind> {
    Some(match result {
        EResult::RateLimitExceeded
        | EResult::LimitExceeded
        | EResult::AccountLimitExceeded
        | EResult::AccountActivityLimitExceeded => ErrorKind::RateLimited,
        EResult::AccessDenied => ErrorKind::AccessDenied,
        EResult::FileNotFound => ErrorKind::NotFound,
        EResult::NoConnection
        | EResult::Busy
        | EResult::ServiceUnavailable
        | EResult::TryAnotherCM => ErrorKind::NetworkTimeout,
        _ => return None,
    })
}

fn network_kind(err: &NetworkError) -> Option<ErrorKind> {
    match err {
        NetworkError::ApiError(result) => eresult_kind(*result),
        NetworkError::Timeout | NetworkError::EOF => Some(ErrorKind::NetworkTimeout),
        _ => None,
    }
}

fn login_kind(err: &LoginError) -> Option<ErrorKind> {
    match err {
        LoginError::RateLimited => Some(ErrorKind::RateLimited),
        LoginError::InvalidCredentials
        | LoginError::SteamGuardRequired
        | LoginError::UnavailableAccount => Some(ErrorKind::AuthRequired),
        LoginError::Unknown(result) => eresult_kind(*result),
        _ => None,
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
