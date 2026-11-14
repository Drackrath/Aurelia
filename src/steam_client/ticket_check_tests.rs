use super::*;
use std::sync::Mutex;

#[derive(Default)]
struct Fake {
    lender: Option<u32>,
    lender_busy: bool,
    reply: Option<(i32, Vec<u8>)>,
    log: Mutex<Vec<String>>,
}

impl TicketGate for Fake {
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
        self.reply
            .clone()
            .ok_or_else(|| anyhow!("no reply from Steam"))
    }

    async fn clear(&self, launch: u64) {
        self.log.lock().unwrap().push(format!("clear {launch}"));
    }
}

#[tokio::test]
async fn ticket_issued_passes_and_clears() {
    let fake = Fake {
        reply: Some((1, vec![1, 2])),
        ..Fake::default()
    };
    check_ticket(&fake, 480, "Spacewar").await.unwrap();
    assert_eq!(*fake.log.lock().unwrap(), ["report 480 None", "clear 5"]);
}

#[tokio::test]
async fn borrowed_game_reports_lender_and_refuses_empty() {
    let fake = Fake {
        lender: Some(77),
        reply: Some((1, Vec::new())),
        ..Fake::default()
    };
    let err = check_ticket(&fake, 945360, "Among Us")
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
    // Report cleared on refusal.
    assert_eq!(
        *fake.log.lock().unwrap(),
        ["report 945360 Some(77)", "clear 5"]
    );
}

#[tokio::test]
async fn eresult_and_errors_are_named() {
    let fake = Fake {
        reply: Some((15, Vec::new())),
        ..Fake::default()
    };
    let err = check_ticket(&fake, 480, "X").await.unwrap_err().to_string();
    assert!(
        err.contains("EResult 15") && err.contains("owned by this account"),
        "{err}"
    );
    let fake = Fake::default();
    let err = check_ticket(&fake, 480, "X").await.unwrap_err().to_string();
    assert!(err.contains("no reply from Steam"), "{err}");
    assert_eq!(*fake.log.lock().unwrap(), ["report 480 None", "clear 5"]);
}

#[tokio::test]
async fn busy_lender_library_refuses_before_reporting() {
    let fake = Fake {
        lender_busy: true,
        reply: Some((1, vec![1])),
        ..Fake::default()
    };
    let err = check_ticket(&fake, 945360, "Among Us").await.unwrap_err();
    assert!(err.to_string().contains("Refusing to launch"), "{err}");
    assert!(fake.log.lock().unwrap().is_empty());
}
