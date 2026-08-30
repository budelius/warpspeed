use warp_core::SessionId;

use super::*;
use crate::local_tty::supervisor::message::SessionMetadata;
use crate::local_tty::screen::ScreenSnapshot;

fn snapshot() -> ScreenSnapshot {
    ScreenSnapshot {
        columns: 80,
        rows: 24,
        repaint: b"\x1b[H\x1b[2Jhello".to_vec(),
    }
}

fn metadata() -> SessionMetadata {
    SessionMetadata {
        shell: "bash".to_owned(),
        working_directory: "/home/user".to_owned(),
        title: "build".to_owned(),
    }
}

fn detach() -> Message {
    Message::Detach {
        session_id: SessionId::from(7u64),
        snapshot: snapshot(),
        metadata: metadata(),
    }
}

/// A detach carries the screen, and the screen is the largest thing on the
/// wire. If framing truncates or mis-lengths it, a reattached pane paints
/// garbage, so round-tripping the whole message is the property to hold.
#[test]
fn a_detach_message_round_trips_through_framing() {
    let message = detach();

    let mut buffer = Vec::new();
    write_message(&mut buffer, &message).expect("write should succeed");

    let decoded = read_message(&mut buffer.as_slice())
        .expect("read should succeed")
        .expect("a whole message was written");

    assert_eq!(message, decoded);
}

/// The reader must not treat a partial write as a whole message. A transport
/// delivers bytes when it feels like it, and a half-read snapshot that decoded
/// as success would repaint a pane with a truncated screen.
#[test]
fn a_partial_message_reads_as_none_rather_than_a_short_one() {
    let mut buffer = Vec::new();
    write_message(&mut buffer, &detach()).expect("write should succeed");
    buffer.truncate(buffer.len() - 1);

    let decoded = read_message(&mut buffer.as_slice()).expect("a short read is not an error");

    assert!(
        decoded.is_none(),
        "a truncated message must not decode as a whole one"
    );
}

/// Guards the round-trip test against passing vacuously. If `write_message`
/// produced nothing and `read_message` returned a default, equality could still
/// hold; this pins that real bytes are produced and that the length prefix
/// describes them.
#[test]
fn framing_writes_a_length_prefix_describing_the_payload() {
    let mut buffer = Vec::new();
    write_message(&mut buffer, &detach()).expect("write should succeed");

    assert!(
        buffer.len() > LENGTH_PREFIX_BYTES,
        "framing produced no payload"
    );

    let mut header = [0u8; LENGTH_PREFIX_BYTES];
    header.copy_from_slice(&buffer[..LENGTH_PREFIX_BYTES]);

    assert_eq!(
        u64::from_be_bytes(header) as usize,
        buffer.len() - LENGTH_PREFIX_BYTES,
        "the length prefix does not describe the payload that follows"
    );
}

/// Two messages written back to back must both come out, in order. A reader
/// that consumed the whole buffer for the first message would lose the second.
#[test]
fn consecutive_messages_are_read_in_order() {
    let mut buffer = Vec::new();
    write_message(&mut buffer, &Message::Ping).expect("write should succeed");
    write_message(&mut buffer, &detach()).expect("write should succeed");

    let mut stream = buffer.as_slice();
    let first = read_message(&mut stream).expect("read should succeed");
    let second = read_message(&mut stream).expect("read should succeed");

    assert_eq!(first, Some(Message::Ping));
    assert_eq!(second, Some(detach()));
}
