use super::super::*;

use crate::app::output::{OutputCtx, scoped};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

#[test]
fn absolute_dirs_are_tidied() {
    let got = absolute_prefix_dir(PathBuf::from("/g/./p/../Elden/")).unwrap();
    assert_eq!(got, Path::new("/g/Elden"));
}

#[test]
fn empty_dir_is_rejected() {
    assert!(absolute_prefix_dir(PathBuf::new()).is_err());
}

#[tokio::test]
async fn relative_dirs_use_the_client_cwd() {
    let (out_tx, _out_rx) = mpsc::unbounded_channel();
    let (_in_tx, in_rx) = mpsc::unbounded_channel();
    let ctx = OutputCtx::new(out_tx, in_rx, Some(PathBuf::from("/home/u/shots")));
    let (rel, dot) = scoped(ctx, async {
        (
            absolute_prefix_dir(PathBuf::from("rel")).unwrap(),
            absolute_prefix_dir(PathBuf::from(".")).unwrap(),
        )
    })
    .await;
    assert_eq!(rel, Path::new("/home/u/shots/rel"));
    assert_eq!(dot, Path::new("/home/u/shots"));
}
