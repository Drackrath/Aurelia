use super::*;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Fake {
    lender: Option<u32>,
    lender_busy: bool,
    reply: Option<(i32, Vec<u8>)>,
    /// Ticket request never answers.
    hang: bool,
    log: Mutex<Vec<String>>,
    cleared: tokio::sync::Notify,
}

type Gate = Arc<Fake>;

impl TicketGate for Gate {
    async fn lender(&self, app_id: u32) -> Result<Option<u32>> {
        if self.lender_busy {
            return Err(super::super::family_lock::LibraryInUse {
                app_id,
                lender: 9,
                by: 8,
            }
            .into());
        }
        Ok(self.lender)
    }

    async fn report(&self, launch: Launch) -> Result<u64> {
        self.log
            .lock()
            .unwrap()
            .push(format!("report {} {:?}", launch.app_id, launch.owner));
        Ok(5)
    }

    async fn ticket(&self, _app_id: u32) -> Result<(i32, Vec<u8>)> {
        if self.hang {
            std::future::pending::<()>().await;
        }
        self.reply
            .clone()
            .ok_or_else(|| anyhow!("no reply from Steam"))
    }

    async fn clear(&self, launch: u64) {
        self.log.lock().unwrap().push(format!("clear {launch}"));
        self.cleared.notify_one();
    }
}

fn gate(fake: Fake) -> Gate {
    Arc::new(fake)
}

#[tokio::test]
async fn ticket_issued_passes_and_clears() {
    let g = gate(Fake {
        reply: Some((1, vec![1, 2])),
        ..Fake::default()
    });
    check_ticket(&g, 480, "Spacewar").await.unwrap();
    assert_eq!(*g.log.lock().unwrap(), ["report 480 None", "clear 5"]);
}

#[tokio::test]
async fn borrowed_game_reports_lender_and_refuses_empty() {
    let g = gate(Fake {
        lender: Some(77),
        reply: Some((1, Vec::new())),
        ..Fake::default()
    });
    let err = check_ticket(&g, 945360, "Among Us")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("Among Us (945360)"), "{err}");
    assert!(
        err.contains("borrowed via Family Sharing from account 77"),
        "{err}"
    );
    assert!(err.contains("empty ticket"), "{err}");
    assert!(err.contains("--no-ticket-check"), "{err}");
    assert_eq!(
        *g.log.lock().unwrap(),
        ["report 945360 Some(77)", "clear 5"]
    );
}

#[tokio::test]
async fn eresult_and_errors_are_named() {
    let g = gate(Fake {
        reply: Some((15, Vec::new())),
        ..Fake::default()
    });
    let err = check_ticket(&g, 480, "X").await.unwrap_err().to_string();
    assert!(
        err.contains("EResult 15") && err.contains("owned by this account"),
        "{err}"
    );
    let g = gate(Fake::default());
    let err = check_ticket(&g, 480, "X").await.unwrap_err().to_string();
    assert!(err.contains("no reply from Steam"), "{err}");
    assert_eq!(*g.log.lock().unwrap(), ["report 480 None", "clear 5"]);
}

#[tokio::test]
async fn busy_lender_library_refuses_before_reporting() {
    let g = gate(Fake {
        lender_busy: true,
        reply: Some((1, vec![1])),
        ..Fake::default()
    });
    let err = check_ticket(&g, 945360, "Among Us")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("Refusing to launch"), "{err}");
    // Borrowed wording, lender named.
    assert!(
        err.contains("borrowed via Family Sharing from account 9"),
        "{err}"
    );
    assert!(!err.contains("owned by this account"), "{err}");
    assert!(g.log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancelled_check_still_clears_the_report() {
    let g = gate(Fake {
        hang: true,
        ..Fake::default()
    });
    // Dropped mid-ticket, like Ctrl-C.
    let check = check_ticket(&g, 945360, "Among Us");
    let timed_out = tokio::time::timeout(std::time::Duration::from_millis(50), check).await;
    assert!(timed_out.is_err());
    tokio::time::timeout(std::time::Duration::from_secs(5), g.cleared.notified())
        .await
        .expect("clear ran after cancellation");
    assert_eq!(*g.log.lock().unwrap(), ["report 945360 None", "clear 5"]);
}
