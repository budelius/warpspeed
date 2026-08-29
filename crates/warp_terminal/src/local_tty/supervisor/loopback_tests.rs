use warp_core::SessionId;

use super::*;
use crate::local_tty::screen::ScreenSnapshot;

fn snapshot() -> ScreenSnapshot {
    ScreenSnapshot {
        columns: 80,
        rows: 24,
        repaint: b"\x1b[H\x1b[2Jrestored".to_vec(),
    }
}

/// The contract the remote design rests on: a transport that lends no endpoint
/// still carries every message. If this holds, an SSH or agent-ctrl transport
/// is a matter of moving bytes; if it does not, remote sessions are not
/// reachable from this design at all.
#[test]
fn a_transport_that_lends_no_endpoint_still_carries_messages() {
    let (mut client, mut supervisor) = LoopbackTransport::pair();

    client
        .send(&Message::ListSessions)
        .expect("send should succeed");

    let received = supervisor
        .receive()
        .expect("receive should succeed")
        .expect("a message was sent");

    assert_eq!(received, Message::ListSessions);
}

/// Endpoint lending is a capability, not a guarantee. Callers must handle its
/// absence rather than assuming a descriptor is always available.
#[test]
fn loopback_reports_that_it_cannot_lend_an_endpoint() {
    let (mut client, _supervisor) = LoopbackTransport::pair();

    assert!(
        client.lend_endpoint(SessionId::from(1u64)).is_none(),
        "loopback must report that it has no lending capability"
    );
}

/// A detach carries the screen, the largest message and the one whose loss is
/// most visible. Round-tripping it over a transport, not just through framing,
/// is what shows the two fit together.
#[test]
fn a_screen_snapshot_survives_a_loopback_round_trip() {
    let (mut client, mut supervisor) = LoopbackTransport::pair();

    client
        .send(&Message::Detach {
            session_id: SessionId::from(3u64),
            snapshot: snapshot(),
        })
        .expect("send should succeed");

    let received = supervisor
        .receive()
        .expect("receive should succeed")
        .expect("a message was sent");

    let Message::Detach {
        snapshot: received_snapshot,
        session_id,
    } = received
    else {
        panic!("expected a Detach message, got {received:?}");
    };

    assert_eq!(session_id, SessionId::from(3u64));
    assert_eq!(received_snapshot, snapshot());
}

/// Guards the tests above against passing on an empty channel: receiving with
/// nothing sent must yield `None`, not a default message.
#[test]
fn receiving_with_nothing_sent_yields_nothing() {
    let (_client, mut supervisor) = LoopbackTransport::pair();

    assert!(
        supervisor.receive().expect("receive should succeed").is_none(),
        "an empty channel must not produce a message"
    );
}

/// The buffering property. A message arriving in pieces must not be lost: the
/// bytes of an incomplete message have to stay buffered rather than being
/// consumed and discarded. This is what a real socket does constantly.
#[test]
fn a_message_split_across_arrivals_is_not_lost() {
    let (client, mut supervisor) = LoopbackTransport::pair();

    // Frame a message by hand so it can be delivered in two pieces.
    let mut framed = Vec::new();
    crate::local_tty::supervisor::framing::write_message(
        &mut framed,
        &Message::Detach {
            session_id: SessionId::from(9u64),
            snapshot: snapshot(),
        },
    )
    .expect("write should succeed");

    let (head, tail) = framed.split_at(framed.len() / 2);

    client.outgoing.lock().extend(head.iter().copied());
    assert!(
        supervisor.receive().expect("receive should succeed").is_none(),
        "half a message must not decode"
    );

    client.outgoing.lock().extend(tail.iter().copied());
    let received = supervisor
        .receive()
        .expect("receive should succeed")
        .expect("the message completed and must now decode");

    assert!(
        matches!(received, Message::Detach { session_id, .. } if session_id == SessionId::from(9u64)),
        "expected the split Detach to survive reassembly, got {received:?}"
    );
}

/// Two messages sent back to back both arrive, in order. A receiver that
/// drained the whole buffer for the first would lose the second.
#[test]
fn consecutive_messages_both_arrive_in_order() {
    let (mut client, mut supervisor) = LoopbackTransport::pair();

    client.send(&Message::Ping).expect("send should succeed");
    client
        .send(&Message::ListSessions)
        .expect("send should succeed");

    assert_eq!(supervisor.receive().unwrap(), Some(Message::Ping));
    assert_eq!(supervisor.receive().unwrap(), Some(Message::ListSessions));
    assert_eq!(supervisor.receive().unwrap(), None);
}
