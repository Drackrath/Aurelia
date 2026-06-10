use aes::cipher::block_padding::UnpadError;
use lzma_rs::error::Error as LzmaError;
use reqwest::StatusCode;
use steam_vent::NetworkError;
use tokio::{sync::AcquireError, task::JoinError};
use zip::result::ZipError;

use crate::cdn::manifest::error::ManifestError;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("lack of data: {0}")]
    Eof(String),
    #[error("decompress: {0}")]
    Decompress(String),
    #[error("unexpected: {0}")]
    Unexpected(String),
    #[error("web request - {0}")]
    Request(String),
    #[error("http status - {0}")]
    HttpStatus(StatusCode),
    #[error("{0}")]
    Network(String),
    #[error("malformed vdf - {0}")]
    InvalidVDF(String),
    #[error("manifest {0}")]
    Manifest(#[from] ManifestError),
    #[error("unexpected none")]
    NoneOption,
}

impl From<JoinError> for Error {
    fn from(err: JoinError) -> Self {
        Self::Unexpected(err.to_string())
    }
}

impl From<ZipError> for Error {
    fn from(err: ZipError) -> Self {
        Self::Decompress(err.to_string())
    }
}

impl From<LzmaError> for Error {
    fn from(err: LzmaError) -> Self {
        Self::Decompress(err.to_string())
    }
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Self::Request(err.to_string())
    }
}

impl From<NetworkError> for Error {
    fn from(err: NetworkError) -> Self {
        Self::Network(err.to_string())
    }
}

impl From<keyvalues_parser::error::Error> for Error {
    fn from(err: keyvalues_parser::error::Error) -> Self {
        Self::InvalidVDF(err.to_string())
    }
}

impl From<UnpadError> for Error {
    fn from(err: UnpadError) -> Self {
        Self::Unexpected(err.to_string())
    }
}

impl From<AcquireError> for Error {
    fn from(err: AcquireError) -> Self {
        Self::Unexpected(err.to_string())
    }
}
