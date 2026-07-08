//! Command handlers, split by domain.

pub(crate) mod common;
pub(crate) mod auth;
pub(crate) mod library;
pub(crate) mod collections;
pub(crate) mod social;
pub(crate) mod market;
pub(crate) mod install;
pub(crate) mod launch;
pub(crate) mod info;
pub(crate) mod config;
pub(crate) mod plugins;
pub(crate) mod scripts;
pub(crate) mod runtimes;
pub(crate) mod cloud;
pub(crate) mod workshop;

pub(crate) use self::common::*;
pub(crate) use self::auth::*;
pub(crate) use self::library::*;
pub(crate) use self::collections::*;
pub(crate) use self::social::*;
pub(crate) use self::market::*;
pub(crate) use self::install::*;
pub(crate) use self::launch::*;
pub(crate) use self::info::*;
pub(crate) use self::config::*;
pub(crate) use self::plugins::*;
pub(crate) use self::scripts::*;
pub(crate) use self::runtimes::*;
pub(crate) use self::cloud::*;
pub(crate) use self::workshop::*;
