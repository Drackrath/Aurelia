//! `microtxn` command handlers.

use crate::commands::common::*;
use crate::daemon;

use anyhow::{Result, bail};
use aurelia::steam_client::microtxn;

/// Purchases live in the daemon.
fn require_daemon() -> Result<()> {
    if !daemon::in_daemon() {
        bail!("purchase prompts are held by the session daemon; it is not running");
    }
    Ok(())
}

/// `microtxn list`.
pub(crate) async fn cmd_list(json: bool) -> Result<()> {
    require_daemon()?;
    let pending = microtxn::pending();
    if json {
        let rows: Vec<_> = pending
            .iter()
            .map(|p| {
                serde_json::json!({
                    "app_id": p.app_id,
                    "order_id": p.order_id,
                    "approval_url": p.approval_url(),
                })
            })
            .collect();
        print_json(&serde_json::json!({ "pending": rows }));
        return Ok(());
    }
    if pending.is_empty() {
        cli_println!("No purchases awaiting approval.");
    }
    for p in &pending {
        cli_println!(
            "app {}  order {}  {}",
            p.app_id,
            p.order_id,
            p.approval_url()
        );
    }
    Ok(())
}

/// `microtxn approve|deny <order>`.
pub(crate) async fn cmd_decide(order: u64, authorized: bool, json: bool) -> Result<()> {
    require_daemon()?;
    let request = daemon::decide_microtxn(order, authorized)?;
    if json {
        print_json(&serde_json::json!({
            "app_id": request.app_id,
            "order_id": request.order_id,
            "authorized": authorized,
        }));
    } else if authorized {
        cli_println!(
            "Told app {} the purchase is approved. Steam completes it only if you approved it on the checkout page.",
            request.app_id
        );
    } else {
        cli_println!("Told app {} to cancel the purchase.", request.app_id);
    }
    Ok(())
}
