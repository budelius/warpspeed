use std::os::unix::net::UnixStream;
use std::time::Duration;

use instant::Instant;
use warp_core::SessionId;

use super::*;
use crate::local_tty::screen::ScreenSnapshot;
use crate::local_tty::supervisor::message::{AttachedSession, SessionMetadata, SessionSummary};

fn pair() -> (UnixSocketTransport, UnixSocketTransport) {
    let (a, b) = UnixStream::pair().expect("a socket pair must be creatable");
    (UnixSocketTransport::new(a), UnixSocketTransport::new(b))
}

fn snapshot() -> ScreenSnapshot {
    ScreenSnapshot {
        columns: 100,
        rows: 30,
        repaint: b"\x1b[H\x1b[2Jhello".to_vec(),
    }
}

#[test]
fn a_message_carrying_no_descriptor_round_trips() {
    let (mut client, mut supervisor) = pair();
    let sent = Message::Detach {
        session_id: SessionId::from(7u64),
        snapshot: snapshot(),
        metadata: SessionMetadata {
            shell: "bash".to_owned(),
            working_directory: "/home/me".to_owned(),
            title: "Claude Code".to_owned(),
            input_editor_active: false,
            bootstrap: None,
        },
    };

    client.send(&sent).expect("sending must succeed");

    assert_eq!(supervisor.receive().unwrap(), Some(sent));
}

/// The load-bearing behaviour: the descriptor the supervisor lends has to
/// arrive as a descriptor the *receiver* can use, not as the number it had in
/// the sender.
#[test]
fn an_endpoint_arrives_as_a_descriptor_the_receiver_owns() {
    let (mut supervisor, mut client) = pair();
    let (lent, mut other_end) = UnixStream::pair().expect("a socket pair must be creatable");

    supervisor
        .send_with_endpoint(
            &Message::SpawnResponse {
                result: Ok(Endpoint {
                    pid: 4242,
                    raw: lent.as_raw_fd() as i64,
                }),
            },
            Some(lent.as_raw_fd()),
        )
        .expect("sending must succeed");

    let Some(Message::SpawnResponse {
        result: Ok(endpoint),
    }) = client.receive().unwrap()
    else {
        panic!("expected a spawn response carrying an endpoint");
    };

    assert_eq!(endpoint.pid, 4242);
    // Proving it is genuinely usable, not merely a plausible number: writing to
    // the received descriptor has to come out of the other end of the pair the
    // supervisor lent.
    let received = unsafe { UnixStream::from_raw_fd(endpoint.raw as RawFd) };
    write_all(&received, b"through the lent endpoint");
    let mut buffer = [0u8; 25];
    std::io::Read::read_exact(&mut other_end, &mut buffer).expect("the lent pair must carry bytes");
    assert_eq!(&buffer, b"through the lent endpoint");
}

/// Messages are framed, so two sent back to back must not be read as one.
#[test]
fn consecutive_messages_are_read_in_order() {
    let (mut client, mut supervisor) = pair();

    client.send(&Message::Ping).unwrap();
    client
        .send(&Message::ListSessionsResponse {
            sessions: vec![SessionSummary {
                session_id: SessionId::from(1u64),
                metadata: SessionMetadata::default(),
                detached_for_seconds: 5,
                attached: false,
            }],
        })
        .unwrap();

    assert_eq!(supervisor.receive().unwrap(), Some(Message::Ping));
    assert!(matches!(
        supervisor.receive().unwrap(),
        Some(Message::ListSessionsResponse { .. })
    ));
}

/// A response that says it has an endpoint but arrives without a descriptor is
/// a protocol error. Accepting it would hand the pane descriptor 0 -- its own
/// standard input -- as the terminal it reads.
#[test]
fn an_endpoint_without_a_descriptor_is_refused() {
    let (mut supervisor, mut client) = pair();

    supervisor
        .send_with_endpoint(
            &Message::AttachResponse {
                result: Ok(AttachedSession {
                    snapshot: snapshot(),
                    buffered_output: Vec::new(),
                    sequence: 0,
                    output_was_dropped: false,
                    endpoint: Some(Endpoint { pid: 1, raw: 3 }),
                    metadata: SessionMetadata::default(),
                }),
            },
            None,
        )
        .expect("sending must succeed");

    assert!(client.receive().is_err());
}

/// A client that dies mid-exchange must not park the supervisor's loop for the
/// whole timeout: nothing feeds the detached terminals while it waits.
#[test]
fn waiting_on_a_peer_that_hung_up_returns_at_once() {
    let (mut supervisor, client) = pair();
    drop(client);

    let started = Instant::now();
    let waited = supervisor.receive_timeout(Duration::from_secs(15));

    assert!(waited.is_err());
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "waited {:?} on a closed peer",
        started.elapsed()
    );
}

fn write_all(stream: &UnixStream, bytes: &[u8]) {
    use std::io::Write;
    (&mut &*stream)
        .write_all(bytes)
        .expect("write must succeed");
}
