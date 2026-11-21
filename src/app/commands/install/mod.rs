//! `install` command handlers.

use crate::app::cli::*;
use crate::app::commands::common::*;

use anyhow::{Context, Result, bail};
use aurelia::core::config::load_launcher_config;
use aurelia::core::error::{ErrorKind, TypedError, classify};
use aurelia::core::models::{DepotPlatform, DownloadState, LibraryGame};
use aurelia::library::backlog;
use aurelia::steam_client::{InstallOptions, SteamClient};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

mod control;
mod duplicates;
mod manifests;
mod relocate;
mod run;
mod update;

pub(crate) use control::*;
pub(crate) use duplicates::*;
pub(crate) use manifests::*;
pub(crate) use relocate::*;
pub(crate) use run::*;
pub(crate) use update::*;

#[cfg(test)]
mod tests;
