use super::*;

const KNOWN_AUTH_PART: &str = "140000000102030405060708090a0b0c0d0e0f1011121314\
                               18000000010000000200000007\
                               7100cb0000000040e2010003000000";
const KNOWN_CRC: u32 = 0x4128_a365;

fn token() -> Vec<u8> {
    (1..=20).collect()
}

fn known_header() -> SessionHeader {
    SessionHeader {
        ticket_type: TICKET_TYPE_SESSION,
        external_ip: Ipv4Addr::new(203, 0, 113, 7),
        connected_ms: 123_456,
        connection_count: 3,
    }
}

fn known_hex() -> String {
    KNOWN_AUTH_PART.split_whitespace().collect()
}

fn tokens_msg(n: u8, keep: Option<u32>) -> CMsgClientGameConnectTokens {
    let mut msg = CMsgClientGameConnectTokens::new();
    msg.tokens = (0..n).map(|i| vec![i; GC_TOKEN_LEN]).collect();
    if let Some(keep) = keep {
        msg.set_max_tokens_to_keep(keep);
    }
    msg
}

fn complete(crc: u32, response: u32, estate: u32) -> CMsgClientTicketAuthComplete {
    let mut msg = CMsgClientTicketAuthComplete::new();
    msg.set_ticket_crc(crc);
    msg.set_eauth_session_response(response);
    msg.set_estate(estate);
    msg.set_steam_id(76_561_197_960_287_930);
    msg.set_game_id(480);
    msg
}

#[test]
fn auth_part_matches_known_bytes() {
    let part = build_auth_part(&token(), &known_header());
    assert_eq!(part.len(), AUTH_PART_LEN);
    assert_eq!(hex::encode(&part), known_hex());
    assert_eq!(ticket_crc(&part), KNOWN_CRC);
}

#[test]
fn session_ticket_appends_ownership() {
    let part = build_auth_part(&token(), &known_header());
    let ticket = build_session_ticket(&part, &[0xaa, 0xbb, 0xcc]);
    assert_eq!(
        hex::encode(&ticket),
        format!("{}03000000aabbcc", known_hex())
    );
}

#[test]
fn parse_round_trips() {
    let part = build_auth_part(&token(), &known_header());
    let ticket = build_session_ticket(&part, b"owner");
    let parsed = parse_session_ticket(&ticket).expect("well formed");
    assert_eq!(parsed.gc_token, token().as_slice());
    assert_eq!(parsed.header, known_header());
    assert_eq!(parsed.auth_part, part.as_slice());
    assert_eq!(parsed.ownership_ticket, b"owner");
}

#[test]
fn parse_rejects_malformed() {
    let part = build_auth_part(&token(), &known_header());
    let ticket = build_session_ticket(&part, b"owner");
    assert!(parse_session_ticket(&ticket[..ticket.len() - 1]).is_none());
    assert!(parse_session_ticket(&ticket[..10]).is_none());
    let mut bad_header = ticket.clone();
    bad_header[24] = 23;
    assert!(parse_session_ticket(&bad_header).is_none());
    let mut bad_token = ticket;
    bad_token[0] = 8;
    assert!(parse_session_ticket(&bad_token).is_none());
}

#[test]
fn header_ip_handles_v6() {
    let v4 = Ipv4Addr::new(1, 2, 3, 4);
    assert_eq!(header_ip(IpAddr::V4(v4)), v4);
    assert_eq!(header_ip(IpAddr::V6(v4.to_ipv6_mapped())), v4);
    assert_eq!(
        header_ip("2001:db8::1".parse().unwrap()),
        Ipv4Addr::UNSPECIFIED
    );
}

#[test]
fn web_api_secret_has_prefix_and_nul() {
    let kind = TicketKind::WebApi(Some("myservice".into()));
    assert_eq!(kind.server_secret().unwrap(), b"str:myservice\0");
    assert_eq!(TicketKind::WebApi(None).server_secret(), None);
    assert_eq!(
        TicketKind::WebApi(Some(String::new())).server_secret(),
        None
    );
    assert_eq!(TicketKind::Session.server_secret(), None);
}

#[test]
fn tokens_trimmed_to_keep() {
    let mut state = AuthTickets::new(Instant::now());
    state.absorb_tokens(&tokens_msg(3, None));
    assert_eq!(state.tokens_left(), 3);
    state.absorb_tokens(&tokens_msg(4, Some(5)));
    assert_eq!(state.tokens_left(), 5);
    state.absorb_tokens(&tokens_msg(1, Some(0)));
    assert_eq!(state.tokens_left(), 6);
}

#[test]
fn issue_fails_closed_without_tokens() {
    let mut state = AuthTickets::new(Instant::now());
    let issued = state.issue(
        480,
        1,
        &TicketKind::Session,
        Ipv4Addr::LOCALHOST,
        Instant::now(),
    );
    assert!(issued.is_none());
    assert!(state.tickets().is_empty());
}

#[test]
fn issue_uses_token_and_counts() {
    let start = Instant::now();
    let mut state = AuthTickets::new(start);
    state.absorb_tokens(&tokens_msg(2, None));
    let ip = Ipv4Addr::new(203, 0, 113, 7);
    let later = start + Duration::from_millis(1500);
    let a = state
        .issue(480, 7, &TicketKind::Session, ip, later)
        .unwrap();
    let b = state
        .issue(480, 7, &TicketKind::WebApi(Some("svc".into())), ip, later)
        .unwrap();
    assert_eq!((a.handle, b.handle), (1, 2));
    assert_eq!(state.tokens_left(), 0);

    let ta = build_session_ticket(&a.auth_part, b"x");
    let pa = parse_session_ticket(&ta).unwrap();
    assert_eq!(pa.gc_token, vec![0; GC_TOKEN_LEN].as_slice());
    assert_eq!(pa.header.ticket_type, TICKET_TYPE_SESSION);
    assert_eq!(pa.header.connected_ms, 1500);
    assert_eq!(pa.header.connection_count, 1);
    assert_eq!(pa.header.external_ip, ip);
    assert_eq!(a.crc, ticket_crc(&a.auth_part));

    let tb = build_session_ticket(&b.auth_part, b"x");
    let pb = parse_session_ticket(&tb).unwrap();
    assert_eq!(pb.gc_token, vec![1; GC_TOKEN_LEN].as_slice());
    assert_eq!(pb.header.ticket_type, TICKET_TYPE_WEB_API);
    assert_eq!(pb.header.connection_count, 2);
    assert_eq!(b.server_secret.as_deref(), Some(&b"str:svc\0"[..]));
}

#[test]
fn auth_list_lists_active_tickets() {
    let mut state = AuthTickets::new(Instant::now());
    state.absorb_tokens(&tokens_msg(3, None));
    let ip = Ipv4Addr::LOCALHOST;
    let now = Instant::now();
    let a = state.issue(480, 9, &TicketKind::Session, ip, now).unwrap();
    let b = state.issue(570, 9, &TicketKind::Session, ip, now).unwrap();
    let c = state
        .issue(480, 9, &TicketKind::WebApi(Some("svc".into())), ip, now)
        .unwrap();

    let list = state.auth_list(None);
    assert_eq!(list.tokens_left(), 0);
    assert_eq!(list.message_sequence(), 1);
    assert_eq!(list.last_request_seq(), 0);
    assert_eq!(list.last_request_seq_from_server(), 0);
    assert_eq!(list.app_ids, vec![480, 570]);
    assert_eq!(list.tickets.len(), 3);
    let first = &list.tickets[0];
    assert_eq!(first.estate(), 0);
    assert_eq!(first.steamid(), 0);
    assert_eq!(first.gameid(), 480);
    assert_eq!(first.h_steam_pipe(), 9);
    assert_eq!(first.ticket_crc(), a.crc);
    assert_eq!(first.ticket(), a.auth_part.as_slice());
    assert!(!first.has_server_secret());
    assert_eq!(list.tickets[1].ticket_crc(), b.crc);
    assert_eq!(list.tickets[2].server_secret(), b"str:svc\0");

    let mut ack = CMsgClientAuthListAck::new();
    ack.set_message_sequence(42);
    state.on_ack(&ack);

    assert_eq!(state.cancel(b.handle), Some(570));
    assert_eq!(state.cancel(b.handle), None);
    let list = state.auth_list(Some(570));
    assert_eq!(list.last_request_seq(), 1);
    assert_eq!(list.last_request_seq_from_server(), 42);
    assert_eq!(list.message_sequence(), 2);
    assert_eq!(list.app_ids, vec![480, 570]);
    let crcs: Vec<u32> = list.tickets.iter().map(|t| t.ticket_crc()).collect();
    assert_eq!(crcs, vec![a.crc, c.crc]);
}

#[test]
fn auth_list_encodes_as_protobuf() {
    let mut state = AuthTickets::new(Instant::now());
    state.absorb_tokens(&tokens_msg(1, None));
    let t = state
        .issue(
            480,
            1,
            &TicketKind::Session,
            Ipv4Addr::LOCALHOST,
            Instant::now(),
        )
        .unwrap();
    let bytes = state.auth_list(None).write_to_bytes().unwrap();
    let back = CMsgClientAuthList::parse_from_bytes(&bytes).unwrap();
    assert_eq!(back.tickets[0].ticket(), t.auth_part.as_slice());
    assert_eq!(back.tickets[0].ticket_crc(), t.crc);
    assert_eq!(back.app_ids, vec![480]);
}

#[test]
fn complete_ok_keeps_ticket() {
    let mut state = AuthTickets::new(Instant::now());
    state.absorb_tokens(&tokens_msg(1, None));
    let t = state
        .issue(
            480,
            1,
            &TicketKind::Session,
            Ipv4Addr::LOCALHOST,
            Instant::now(),
        )
        .unwrap();
    let event = state.on_complete(&complete(t.crc, 0, 2));
    assert_eq!(event.handle, Some(t.handle));
    assert_eq!(event.game_id, 480);
    assert_eq!(state.tickets()[0].estate, 2);
    assert_eq!(state.auth_list(None).tickets[0].estate(), 2);
    assert_eq!(state.take_events(), vec![event]);
    assert!(state.take_events().is_empty());
}

#[test]
fn complete_failure_drops_ticket() {
    let mut state = AuthTickets::new(Instant::now());
    state.absorb_tokens(&tokens_msg(1, None));
    let t = state
        .issue(
            480,
            1,
            &TicketKind::Session,
            Ipv4Addr::LOCALHOST,
            Instant::now(),
        )
        .unwrap();
    let event = state.on_complete(&complete(t.crc, 6, 1));
    assert_eq!(event.response, 6);
    assert!(state.tickets().is_empty());
}

#[test]
fn complete_unknown_crc_is_reported() {
    let mut state = AuthTickets::new(Instant::now());
    let event = state.on_complete(&complete(0xdead_beef, 0, 1));
    assert_eq!(event.handle, None);
    assert_eq!(state.take_events().len(), 1);
}

#[test]
fn events_are_bounded() {
    let mut state = AuthTickets::new(Instant::now());
    for crc in 0..(MAX_EVENTS as u32 + 5) {
        state.on_complete(&complete(crc, 0, 1));
    }
    let events = state.take_events();
    assert_eq!(events.len(), MAX_EVENTS);
    assert_eq!(events[0].ticket_crc, 5);
}

#[test]
fn store_resets_on_new_session() {
    record_gc_tokens(-77, &tokens_msg(2, None));
    assert_eq!(with_store(-77, |s| s.tokens_left()), 2);
    assert_eq!(with_store(-78, |s| s.tokens_left()), 0);
}
