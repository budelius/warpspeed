use super::*;

#[test]
fn v6_lists_decode_multiple_sessions_without_inventing_agent_identity() {
    // No shell pid or UUID fields separate v6 rows; v7 decoding would consume the next row.
    let payload = hex::decode(concat!(
        "0d0000000200000000000000",
        "ffffffffffffffff0000000000000000000000000000000000000000000000000000070000000000000000",
        "2a000000000000000000000000000000000000000000000000000000000000000000030000000000000001"
    ))
    .unwrap();
    let Message::ListSessionsResponse { sessions } = decode(&payload, 6).unwrap() else {
        panic!("expected sessions");
    };
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].session_id, SessionId::from(u64::MAX));
    assert!(!sessions[0].attached);
    assert_eq!(sessions[0].detached_for_seconds, 7);
    assert_eq!(sessions[0].shell_pid, 0);
    assert_eq!(sessions[0].session_uuid_hex, "");
    assert_eq!(sessions[1].session_id, SessionId::from(42));
    assert!(sessions[1].attached);
    assert_eq!(sessions[1].detached_for_seconds, 3);
}

#[test]
fn unsupported_versions_are_not_guessed_from_the_payload() {
    let ping = bincode::serialize(&Message::Ping).unwrap();
    assert!(decode(&ping, 5).is_err());
    assert!(decode(&ping, PROTOCOL_VERSION + 1).is_err());
    assert!(
        encode(
            &Message::Kill {
                session_id: SessionId::from(7)
            },
            5
        )
        .is_err()
    );
}

#[test]
fn truncated_v6_list_does_not_decode_as_an_empty_list() {
    let payload = hex::decode("0d00000001000000000000002a00000000000000").unwrap();
    assert!(decode(&payload, 6).is_err());
}
