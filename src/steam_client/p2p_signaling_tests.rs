use super::*;

const ME: u64 = 76_561_197_960_287_930;
const PEER: u64 = 76_561_197_960_287_931;

#[test]
fn outgoing_signal_names_both_ends() {
    let msg = signal_message(480, ME, PEER, vec![1, 2, 3]);
    assert_eq!(msg.app_id(), 480);
    assert_eq!(msg.steam_id_src(), ME);
    assert_eq!(msg.steam_id_dest(), PEER);
    assert_eq!(msg.rendezvous(), [1, 2, 3]);
    assert!(!msg.has_candidate());
}

#[test]
fn incoming_signal_is_filtered_by_app() {
    let msg = signal_message(480, PEER, ME, vec![9]);
    assert_eq!(
        signal_event(&msg, 480),
        Some(SignalEvent::Signal {
            from: PEER,
            rendezvous: vec![9]
        })
    );
    assert_eq!(signal_event(&msg, 570), None);
    // Legacy candidate-only messages.
    let empty = signal_message(480, PEER, ME, Vec::new());
    assert_eq!(signal_event(&empty, 480), None);
}

#[test]
fn failure_carries_reason() {
    let mut msg = CMsgClientP2PConnectionFailInfo::new();
    msg.set_app_id(480);
    msg.set_steam_id_src(PEER);
    msg.set_ep2p_session_error(3);
    msg.set_close_reason(5003);
    msg.set_close_message("gone".into());
    assert_eq!(
        failure_event(&msg, 480),
        Some(SignalEvent::Failed {
            from: PEER,
            error: 3,
            close_reason: 5003,
            message: "gone".into()
        })
    );
    assert_eq!(failure_event(&msg, 1), None);
}
