use warp_core::SessionId;

use super::*;
use crate::local_tty::screen::ScreenSnapshot;
use crate::local_tty::supervisor::message::SessionMetadata;

/// A pipe name unique to one test, so tests can run in parallel without
/// colliding in the shared Windows pipe namespace.
fn test_pipe_name(label: &str) -> String {
    format!(
        r"\\.\pipe\warp-terminal-supervisor-test-{}-{}",
        label,
        std::process::id()
    )
}

fn snapshot() -> ScreenSnapshot {
    ScreenSnapshot {
        columns: 80,
        rows: 24,
        repaint: b"\x1b[H\x1b[2Jrestored".to_vec(),
    }
}

fn metadata() -> SessionMetadata {
    SessionMetadata {
        shell: "bash".to_owned(),
        working_directory: "/home/user".to_owned(),
        title: "build".to_owned(),
    }
}

/// A connected client and supervisor pair.
///
/// Single-threaded on purpose. Because `bind` creates the instance up front, a
/// client can connect before `accept` is called, and `accept` then returns
/// immediately through the `ERROR_PIPE_CONNECTED` path. Doing this with threads
/// invites a test that hangs forever instead of failing, which is exactly what
/// the first version of these tests did.
fn connected_pair(label: &str) -> (PipeTransport, PipeTransport) {
    let name = test_pipe_name(label);
    let mut listener = PipeListener::bind(name.clone()).expect("bind should succeed");
    let client = PipeTransport::connect(&name).expect("connect should succeed");
    let supervisor = listener.accept().expect("accept should succeed");
    (client, supervisor)
}

/// The control channel end to end: a message crosses a real Windows pipe.
#[test]
fn a_message_crosses_a_real_pipe() {
    let (mut client, mut supervisor) = connected_pair("crosses");

    client
        .send(&Message::ListSessions)
        .expect("client send should succeed");

    let received = supervisor.receive().expect("receive should succeed");

    assert_eq!(received, Some(Message::ListSessions));
}

/// A snapshot is the largest thing the channel carries and the one whose
/// truncation would be least visible, so it is worth crossing a real pipe
/// rather than only the framing tests' in-memory buffer.
#[test]
fn a_snapshot_survives_a_real_pipe() {
    let (mut client, mut supervisor) = connected_pair("snapshot");

    client
        .send(&Message::Detach {
            session_id: SessionId::from(11u64),
            snapshot: snapshot(),
            metadata: metadata(),
        })
        .expect("client send should succeed");

    let received = supervisor.receive().expect("receive should succeed");

    let Some(Message::Detach {
        snapshot: received_snapshot,
        session_id,
        ..
    }) = received
    else {
        panic!("expected a Detach message, got {received:?}");
    };

    assert_eq!(session_id, SessionId::from(11u64));
    assert_eq!(received_snapshot, snapshot());
}

/// The channel is bidirectional: the supervisor answers on the same pipe.
#[test]
fn a_reply_travels_back_over_the_same_pipe() {
    let (mut client, mut supervisor) = connected_pair("reply");

    client
        .send(&Message::Ping)
        .expect("client send should succeed");
    assert_eq!(
        supervisor.receive().expect("receive should succeed"),
        Some(Message::Ping)
    );

    supervisor
        .send(&Message::Pong {
            protocol_version: PROTOCOL_VERSION,
        })
        .expect("supervisor send should succeed");

    assert_eq!(
        client.receive().expect("receive should succeed"),
        Some(Message::Pong {
            protocol_version: PROTOCOL_VERSION
        })
    );
}

/// Guards the tests above against a receive that reports success without
/// anything arriving: with a connected but silent peer, receive must return
/// `None` rather than a message.
#[test]
fn receiving_from_a_silent_peer_yields_nothing() {
    let (_client, mut supervisor) = connected_pair("silent");

    assert!(
        supervisor
            .receive()
            .expect("receive should succeed")
            .is_none(),
        "a peer that sent nothing must not produce a message"
    );
}

/// Two messages sent back to back both arrive, in order. A receiver that
/// consumed the whole buffer for the first would lose the second.
#[test]
fn consecutive_messages_both_arrive_in_order() {
    let (mut client, mut supervisor) = connected_pair("consecutive");

    client.send(&Message::Ping).expect("send should succeed");
    client
        .send(&Message::ListSessions)
        .expect("send should succeed");

    assert_eq!(supervisor.receive().unwrap(), Some(Message::Ping));
    assert_eq!(supervisor.receive().unwrap(), Some(Message::ListSessions));
    assert_eq!(supervisor.receive().unwrap(), None);
}

/// Unlike a loopback, this transport really can carry an endpoint, because the
/// supervisor duplicates the PTY handle into the client.
#[test]
fn the_pipe_transport_reports_that_it_lends_endpoints() {
    let (client, supervisor) = connected_pair("lends");

    assert!(client.lends_endpoints());
    assert!(supervisor.lends_endpoints());
}

/// The pipe must exist as soon as `bind` returns. If it were created lazily in
/// `accept`, a client could look for a name the supervisor has advertised but
/// not yet created -- the race that deadlocked the first version of these
/// tests, with the client giving up and the supervisor blocking forever on a
/// connection that never comes.
#[test]
fn the_pipe_exists_as_soon_as_bind_returns() {
    let name = test_pipe_name("eager");

    let _listener = PipeListener::bind(name.clone()).expect("bind should succeed");

    assert!(
        PipeTransport::connect(&name).is_ok(),
        "a client must be able to connect the moment bind returns"
    );
}

/// Connecting to a name nobody is listening on has to fail rather than block.
#[test]
fn connecting_to_a_pipe_that_does_not_exist_fails() {
    let name = test_pipe_name("absent");

    let result = PipeTransport::connect(&name);

    assert!(
        result.is_err(),
        "connecting to an unbound pipe name must fail"
    );
}

/// The pipe name pins two things the design depends on: the protocol version,
/// so a v2 client cannot reach a v1 supervisor, and a state-directory hash, so
/// two installations do not share one supervisor.
#[test]
fn the_supervisor_pipe_name_carries_the_protocol_version() {
    let name = supervisor_pipe_name();

    assert!(
        name.starts_with(r"\\.\pipe\warp-terminal-supervisor-v"),
        "unexpected pipe name {name}"
    );
    assert!(
        name.contains(&format!("-v{PROTOCOL_VERSION}-")),
        "the pipe name must carry the protocol version, got {name}"
    );
    assert_eq!(
        name,
        supervisor_pipe_name(),
        "the pipe name must be stable between calls"
    );
}
