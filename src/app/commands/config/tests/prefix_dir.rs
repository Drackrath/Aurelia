use super::super::*;

use crate::app::output::{OutputCtx, scoped};
use std::path::PathBuf;
use tokio::sync::mpsc;

#[test]
fn absolute_dirs_are_tidied() {
    let base = std::env::temp_dir();
    let got = absolute_prefix_dir(base.join("g/./p/../Elden/")).unwrap();
    assert_eq!(got, base.join("g").join("Elden"));
}

#[test]
fn empty_dir_is_rejected() {
    assert!(absolute_prefix_dir(PathBuf::new()).is_err());
}

#[tokio::test]
async fn relative_dirs_use_the_client_cwd() {
    let (out_tx, _out_rx) = mpsc::unbounded_channel();
    let (_in_tx, in_rx) = mpsc::unbounded_channel();
    let cwd = std::env::temp_dir().join("shots");
    let ctx = OutputCtx::new(out_tx, in_rx, Some(cwd.clone()));
    let (rel, dot) = scoped(ctx, async {
        (
            absolute_prefix_dir(PathBuf::from("rel")).unwrap(),
            absolute_prefix_dir(PathBuf::from(".")).unwrap(),
        )
    })
    .await;
    assert_eq!(rel, cwd.join("rel"));
    assert_eq!(dot, cwd);
}
