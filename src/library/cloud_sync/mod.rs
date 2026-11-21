use anyhow::{Context, Result, anyhow, bail};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use sha1::{Digest, Sha1};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;
use steam_vent::ConnectionTrait;
use steam_vent::connection::Connection;
use steam_vent_proto::steammessages_cloud_steamclient::{
    CCloud_ClientBeginFileUpload_Request, CCloud_ClientBeginFileUpload_Response,
    CCloud_ClientCommitFileUpload_Request, CCloud_ClientCommitFileUpload_Response,
    CCloud_ClientFileDownload_Request, CCloud_ClientFileDownload_Response,
    CCloud_EnumerateUserFiles_Request, CCloud_EnumerateUserFiles_Response,
    ClientCloudFileUploadBlockDetails,
};

mod paths;
mod plan;
mod transfer;
mod transport;
mod types;

pub use paths::*;
use plan::*;
pub use transfer::*;
use transport::*;
pub use types::*;

#[cfg(test)]
mod tests;
