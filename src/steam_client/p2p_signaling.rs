//! P2P signaling, inferred EMsg 5434/5435.
use super::*;
use steam_vent_proto::steammessages_clientserver::{
    CMsgClientP2PConnectionFailInfo, CMsgClientP2PConnectionInfo,
};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

/// Signal for or from a peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalEvent {
    /// Serialized `CMsgSteamNetworkingP2PRendezvous`.
    Signal { from: u64, rendezvous: Vec<u8> },
    /// Steam could not deliver.
    Failed {
        from: u64,
        error: u32,
        close_reason: u32,
        message: String,
    },
}

/// Outgoing 5434 for `to`.
pub fn signal_message(
    app_id: u32,
    from: u64,
    to: u64,
    rendezvous: Vec<u8>,
) -> CMsgClientP2PConnectionInfo {
    let mut msg = CMsgClientP2PConnectionInfo::new();
    msg.set_app_id(app_id);
    msg.set_steam_id_src(from);
    msg.set_steam_id_dest(to);
    msg.set_rendezvous(rendezvous);
    msg
}

/// Incoming 5434 for `app_id`.
pub fn signal_event(msg: &CMsgClientP2PConnectionInfo, app_id: u32) -> Option<SignalEvent> {
    let ours = msg.app_id() == app_id && msg.steam_id_src() != 0;
    (ours && !msg.rendezvous().is_empty()).then(|| SignalEvent::Signal {
        from: msg.steam_id_src(),
        rendezvous: msg.rendezvous().to_vec(),
    })
}

/// Incoming 5435 for `app_id`.
pub fn failure_event(msg: &CMsgClientP2PConnectionFailInfo, app_id: u32) -> Option<SignalEvent> {
    (msg.app_id() == app_id).then(|| SignalEvent::Failed {
        from: msg.steam_id_src(),
        error: msg.ep2p_session_error(),
        close_reason: msg.close_reason(),
        message: msg.close_message().to_owned(),
    })
}

impl SteamClient {
    /// Sends one signal to `to`.
    pub async fn send_p2p_signal(&self, app_id: u32, to: u64, rendezvous: Vec<u8>) -> Result<()> {
        let from = self.steam_id().ok_or_else(|| anyhow!("not logged in"))?;
        self.require_connection()?
            .send(signal_message(app_id, from, to, rendezvous))
            .await
            .map_err(|e| anyhow!("P2P signal not sent: {e}"))
    }

    /// Signals arriving for one app.
    pub fn p2p_signal_events(&self, app_id: u32) -> Result<mpsc::UnboundedReceiver<SignalEvent>> {
        let connection = self.require_connection_owned()?;
        let mut infos = connection.on::<CMsgClientP2PConnectionInfo>();
        let mut fails = connection.on::<CMsgClientP2PConnectionFailInfo>();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    Some(Ok(m)) = infos.next() => signal_event(&m, app_id),
                    Some(Ok(m)) = fails.next() => failure_event(&m, app_id),
                    () = tx.closed() => return,
                    else => return,
                };
                if let Some(event) = event
                    && tx.send(event).is_err()
                {
                    return;
                }
            }
        });
        Ok(rx)
    }
}

#[cfg(test)]
#[path = "p2p_signaling_tests.rs"]
mod tests;
