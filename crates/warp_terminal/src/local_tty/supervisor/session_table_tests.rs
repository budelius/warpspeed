use warp_core::SessionId;

use super::*;
use crate::event_listener::ChannelEventListener;

fn listener() -> ChannelEventListener {
    ChannelEventListener::new_for_test()
}

fn metadata() -> SessionMetadata {
    SessionMetadata {
        shell: "bash".to_owned(),
        working_directory: "/home/user".to_owned(),
        title: "build".to_owned(),
    }
}

/// Detaches a session whose screen has already been fed `before`.
fn table_with_detached_session(session_id: SessionId, before: &[u8], at: u64) -> SessionTable {
    let mut screen = HeadlessScreen::new(40, 6, listener());
    screen.feed(before);

    let mut table = SessionTable::new();
    table.detach(session_id, screen.snapshot(), metadata(), at, listener());
    table
}

/// The contract, end to end: applying the snapshot and then the buffered output
/// must reproduce exactly what a client that never let go would be looking at.
/// This is the same property proved for the screen itself, now carried through
/// the supervisor's own bookkeeping.
#[test]
fn reattaching_reproduces_what_a_client_that_never_detached_would_see() {
    let session_id = SessionId::from(1u64);
    let before = b"before detaching\r\n";
    let after = b"while detached\r\nmore output";

    let mut table = table_with_detached_session(session_id, before, 0);
    table.push_output(session_id, after);

    let attached = table.attach(session_id).expect("attach should succeed");
    assert!(
        !attached.output_was_dropped,
        "nothing should have been dropped in this test"
    );

    let mut reattached = HeadlessScreen::from_snapshot(&attached.snapshot, listener());
    reattached.feed(&attached.buffered_output);

    let mut never_detached = HeadlessScreen::new(40, 6, listener());
    never_detached.feed(before);
    never_detached.feed(after);

    assert_eq!(
        never_detached.visible_text(),
        reattached.visible_text(),
        "a reattached client saw something different from one that never detached"
    );
}

/// Guards the test above against passing on empty screens: the fixture must
/// actually put the "while detached" output somewhere the client can see it.
#[test]
fn buffered_output_actually_carries_what_arrived_while_detached() {
    let session_id = SessionId::from(1u64);
    let mut table = table_with_detached_session(session_id, b"before", 0);
    table.push_output(session_id, b"while detached");

    let attached = table.attach(session_id).expect("attach should succeed");

    assert_eq!(
        attached.buffered_output, b"while detached",
        "output produced while detached was not handed back"
    );
}

#[test]
fn attaching_an_unknown_session_fails() {
    let mut table = SessionTable::new();

    let result = table.attach(SessionId::from(99u64));

    assert!(result.is_err(), "an unknown session must not attach");
}

/// Two clients must not drive one PTY. The second attach has to be refused
/// rather than silently handing out a second borrow of the same session.
#[test]
fn attaching_a_session_twice_fails() {
    let session_id = SessionId::from(1u64);
    let mut table = table_with_detached_session(session_id, b"content", 0);

    table.attach(session_id).expect("first attach should succeed");
    let second = table.attach(session_id);

    assert!(second.is_err(), "a second attach must be refused");
}

/// When the ring has wrapped, the detach-point screen can no longer be caught
/// up. The supervisor must hand back the present and say scrollback was lost,
/// rather than a stream with a hole in it.
#[test]
fn a_wrapped_ring_hands_back_the_current_screen_and_reports_the_loss() {
    let session_id = SessionId::from(1u64);
    let mut table = table_with_detached_session(session_id, b"before", 0);

    // Overrun the ring many times over.
    let flood = vec![b'x'; DEFAULT_RING_CAPACITY_BYTES + 1024];
    table.push_output(session_id, &flood);

    let attached = table.attach(session_id).expect("attach should succeed");

    assert!(
        attached.output_was_dropped,
        "a wrapped ring must report the loss"
    );
    assert!(
        attached.buffered_output.is_empty(),
        "a dropped-output attach sends the current screen instead of a partial stream"
    );
    assert!(
        attached.snapshot.repaint.contains(&b'x'),
        "the snapshot handed back should be the current screen, which holds the flood"
    );
}

#[test]
fn list_reports_how_long_a_session_has_been_detached() {
    let session_id = SessionId::from(1u64);
    let table = table_with_detached_session(session_id, b"content", 100);

    let listed = table.list(160);

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].detached_for_seconds, 60);
    assert_eq!(listed[0].metadata.shell, "bash");
}

/// The drawer must not reshuffle between refreshes, so the order is pinned.
#[test]
fn list_is_ordered_stably_by_session() {
    let mut table = SessionTable::new();
    for id in [3u64, 1, 2] {
        let session_id = SessionId::from(id);
        let screen = HeadlessScreen::new(20, 4, listener());
        table.detach(session_id, screen.snapshot(), metadata(), 0, listener());
    }

    let ids: Vec<u64> = table
        .list(0)
        .iter()
        .map(|summary| summary.session_id.as_u64())
        .collect();

    assert_eq!(ids, vec![1, 2, 3]);
}

/// The TTL exists so forgotten sessions do not accumulate forever.
#[test]
fn reaping_drops_only_sessions_past_the_age_limit() {
    let old = SessionId::from(1u64);
    let recent = SessionId::from(2u64);

    let mut table = table_with_detached_session(old, b"old", 0);
    let screen = HeadlessScreen::new(20, 4, listener());
    table.detach(recent, screen.snapshot(), metadata(), 500, listener());

    let reaped = table.reap_older_than(600, 300);

    assert_eq!(reaped, vec![old]);
    assert_eq!(table.len(), 1, "the recent session must survive");
}

/// An attached session is somebody's live pane. Age must not reap it out from
/// under them.
#[test]
fn reaping_never_drops_an_attached_session() {
    let session_id = SessionId::from(1u64);
    let mut table = table_with_detached_session(session_id, b"content", 0);
    table.attach(session_id).expect("attach should succeed");

    let reaped = table.reap_older_than(100_000, 1);

    assert!(
        reaped.is_empty(),
        "an attached session must not be reaped no matter its age"
    );
    assert_eq!(table.len(), 1);
}

/// Ping carries the protocol version, which is how a client discovers it is
/// talking to a supervisor it cannot speak to.
#[test]
fn ping_is_answered_with_the_protocol_version() {
    let mut table = SessionTable::new();

    let response = table.handle_request(&Message::Ping, 0);

    assert_eq!(
        response,
        Some(Message::Pong {
            protocol_version: crate::local_tty::supervisor::PROTOCOL_VERSION
        })
    );
}

/// Responses are not requests. Handing one back would let two supervisors
/// answer each other in a loop.
#[test]
fn a_response_message_is_not_treated_as_a_request() {
    let mut table = SessionTable::new();

    let response = table.handle_request(&Message::Pong { protocol_version: 2 }, 0);

    assert!(response.is_none(), "a response must not produce a reply");
}
