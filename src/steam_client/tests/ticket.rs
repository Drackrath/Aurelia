use super::super::*;

#[test]
fn encrypted_ticket_refusal_maps_eresult() {
    use steam_vent_proto::steammessages_clientserver::CMsgClientRequestEncryptedAppTicketResponse;

    let mut response = CMsgClientRequestEncryptedAppTicketResponse::new();
    response.set_eresult(15);
    assert_eq!(
        client::extract_encrypted_ticket(&response, 400).unwrap(),
        EncryptedTicketOutcome::Refused(15)
    );

    // Missing eresult defaults to 2.
    let response = CMsgClientRequestEncryptedAppTicketResponse::new();
    assert_eq!(
        client::extract_encrypted_ticket(&response, 400).unwrap(),
        EncryptedTicketOutcome::Refused(2)
    );
}

#[test]
fn encrypted_ticket_success_keeps_envelope() {
    use protobuf::Message;
    use steam_vent_proto::encrypted_app_ticket::EncryptedAppTicket;
    use steam_vent_proto::steammessages_clientserver::CMsgClientRequestEncryptedAppTicketResponse;

    let mut envelope = EncryptedAppTicket::new();
    envelope.set_ticket_version_no(4);
    envelope.set_crc_encryptedticket(0xDEAD_BEEF);
    envelope.set_cb_encrypteduserdata(16);
    envelope.set_cb_encrypted_appownershipticket(96);
    envelope.set_encrypted_ticket(vec![7u8; 128]);

    let mut response = CMsgClientRequestEncryptedAppTicketResponse::new();
    response.set_eresult(1);
    response.encrypted_app_ticket = protobuf::MessageField::some(envelope);

    let EncryptedTicketOutcome::Issued(bytes) =
        client::extract_encrypted_ticket(&response, 945360).unwrap()
    else {
        panic!("expected an issued ticket");
    };

    // Round-trip: every envelope field survives.
    let parsed = EncryptedAppTicket::parse_from_bytes(&bytes).unwrap();
    assert_eq!(parsed.ticket_version_no(), 4);
    assert_eq!(parsed.crc_encryptedticket(), 0xDEAD_BEEF);
    assert_eq!(parsed.cb_encrypteduserdata(), 16);
    assert_eq!(parsed.cb_encrypted_appownershipticket(), 96);
    assert_eq!(parsed.encrypted_ticket(), &[7u8; 128][..]);
}

#[test]
fn encrypted_ticket_ok_without_ticket_errors() {
    use steam_vent_proto::encrypted_app_ticket::EncryptedAppTicket;
    use steam_vent_proto::steammessages_clientserver::CMsgClientRequestEncryptedAppTicketResponse;

    // OK but no envelope at all.
    let mut response = CMsgClientRequestEncryptedAppTicketResponse::new();
    response.set_eresult(1);
    assert!(client::extract_encrypted_ticket(&response, 400).is_err());

    // OK but the ciphertext is empty.
    let mut response = CMsgClientRequestEncryptedAppTicketResponse::new();
    response.set_eresult(1);
    response.encrypted_app_ticket = protobuf::MessageField::some(EncryptedAppTicket::new());
    assert!(client::extract_encrypted_ticket(&response, 400).is_err());
}
