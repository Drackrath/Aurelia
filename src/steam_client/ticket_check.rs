//! Ownership ticket check before stand-in launches.
use super::playing::{GAMES, Launch};
use super::*;
use std::future::Future;

/// Steam calls the check needs.
pub trait TicketGate: Clone + Send + Sync + 'static {
    /// Lender account for borrowed apps.
    fn lender(&self, app_id: u32) -> impl Future<Output = Result<Option<u32>>> + Send;
    /// Report playing; returns launch id.
    fn report(&self, launch: Launch) -> impl Future<Output = Result<u64>> + Send;
    /// EResult and ticket bytes.
    fn ticket(&self, app_id: u32) -> impl Future<Output = Result<(i32, Vec<u8>)>> + Send;
    /// Clear the report.
    fn clear(&self, launch: u64) -> impl Future<Output = ()> + Send;
}

/// `EResult` OK.
const ERESULT_OK: i32 = 1;

/// Hint for the escape hatch.
const ESCAPE: &str = "if the game runs without Steam auth, launch anyway with \
    `aurelia play <app> --no-ticket-check` (or `aurelia config aureliaworks \
    --ticket-check off`)";

/// Refusal text for one outcome.
pub fn refusal(app_id: u32, name: &str, lender: Option<u32>, why: &str) -> String {
    let (kind, todo) = match lender {
        Some(lender) => (
            format!("borrowed via Family Sharing from account {lender}"),
            "ask the lender to check that Family Sharing still includes this game and \
             that they are not playing from their library right now",
        ),
        None => (
            "owned by this account".to_string(),
            "log in with an account that owns the game",
        ),
    };
    format!(
        "Refusing to launch {name} ({app_id}) with the stand-in: Steam issued no app \
         ownership ticket ({why}). The game is {kind}; without a ticket it would hang at \
         login. To fix it, {todo}; {ESCAPE}."
    )
}

/// Clears the report; drop spawns it.
struct ClearOnDrop<G: TicketGate> {
    gate: Option<G>,
    id: u64,
}

impl<G: TicketGate> ClearOnDrop<G> {
    async fn clear(mut self) {
        if let Some(gate) = self.gate.take() {
            gate.clear(self.id).await;
        }
    }
}

impl<G: TicketGate> Drop for ClearOnDrop<G> {
    fn drop(&mut self) {
        let Some(gate) = self.gate.take() else { return };
        let id = self.id;
        // Cancelled mid-check: clear later.
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move { gate.clear(id).await });
            }
            Err(_) => tracing::warn!(id, "could not clear games played: no runtime"),
        }
    }
}

/// Report, ask for the ticket, clear.
pub async fn check_ticket<G: TicketGate>(gate: &G, app_id: u32, name: &str) -> Result<()> {
    let lender = match gate.lender(app_id).await {
        Ok(lender) => lender,
        Err(e) => match e.downcast_ref::<super::family_lock::LibraryInUse>() {
            // Borrowed: name the lender.
            Some(busy) => bail!(refusal(app_id, name, Some(busy.lender), &format!("{e:#}"))),
            None => {
                tracing::warn!(app_id, "Family Sharing owner unknown: {e:#}");
                None
            }
        },
    };
    let id = gate
        .report(Launch {
            app_id,
            pid: None,
            owner: lender,
        })
        .await?;
    // Clears even if cancelled.
    let guard = ClearOnDrop {
        gate: Some(gate.clone()),
        id,
    };
    let reply = gate.ticket(app_id).await;
    guard.clear().await;
    match reply {
        Ok((ERESULT_OK, ticket)) if !ticket.is_empty() => Ok(()),
        Ok((ERESULT_OK, _)) => bail!(refusal(app_id, name, lender, "empty ticket, EResult 1")),
        Ok((eresult, _)) => bail!(refusal(app_id, name, lender, &format!("EResult {eresult}"))),
        Err(e) => bail!(refusal(app_id, name, lender, &format!("{e:#}"))),
    }
}

impl SteamClient {
    /// Ownership ticket with its EResult.
    pub async fn ownership_ticket_reply(&self, app_id: u32) -> Result<(i32, Vec<u8>)> {
        let connection = self.require_connection()?;
        let mut request = CMsgClientGetAppOwnershipTicket::new();
        request.set_app_id(app_id);
        let response: steam_vent_proto::steammessages_clientserver::CMsgClientGetAppOwnershipTicketResponse =
            connection
                .job(request)
                .await
                .context("failed requesting app ownership ticket")?;
        Ok((response.eresult() as i32, response.ticket().to_vec()))
    }

    fn playing_link(
        &self,
    ) -> impl Fn() -> std::future::Ready<Option<SteamClient>> + Send + Sync + 'static {
        let client = self.clone();
        move || std::future::ready(Some(client.clone()))
    }
}

impl TicketGate for SteamClient {
    async fn lender(&self, app_id: u32) -> Result<Option<u32>> {
        self.lender_for(app_id).await
    }

    async fn report(&self, launch: Launch) -> Result<u64> {
        super::playing::start_launch(&GAMES, launch, self.playing_link()).await
    }

    async fn ticket(&self, app_id: u32) -> Result<(i32, Vec<u8>)> {
        self.ownership_ticket_reply(app_id).await
    }

    async fn clear(&self, launch: u64) {
        if let Err(e) = super::playing::stop_launch(&GAMES, launch, &self.playing_link()).await {
            tracing::warn!("could not clear games played: {e:#}");
        }
    }
}

#[cfg(test)]
#[path = "ticket_check_tests.rs"]
mod tests;
