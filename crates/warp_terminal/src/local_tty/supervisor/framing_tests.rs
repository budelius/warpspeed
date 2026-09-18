use warp_core::SessionId;

use super::*;
use crate::local_tty::screen::ScreenSnapshot;
use crate::local_tty::supervisor::message::{
    AttachedSession, PROTOCOL_VERSION, SessionBootstrap, SessionMetadata, SessionSummary,
};
use crate::local_tty::supervisor::transport::Endpoint;

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
        bootstrap: Some(bootstrap()),
    }
}

/// A fully populated shell announcement, so no field of it escapes the
/// encoding test below.
fn bootstrap() -> SessionBootstrap {
    SessionBootstrap {
        hook_session_id: 99,
        shell: "bash".to_owned(),
        user: "user".to_owned(),
        hostname: "host".to_owned(),
        wsl_name: Some("Ubuntu".to_owned()),
        is_subshell: false,
        histfile: Some("/home/user/.bash_history".to_owned()),
        home_dir: Some("/home/user".to_owned()),
        path: Some("/usr/bin".to_owned()),
        cdpath: Some("/srv".to_owned()),
        editor: Some("vi".to_owned()),
        aliases: Some("ll=ls -l".to_owned()),
        abbreviations: Some("g=git".to_owned()),
        function_names: Some("mkcd".to_owned()),
        env_var_names: Some("PATH".to_owned()),
        builtins: Some("cd".to_owned()),
        keywords: Some("if".to_owned()),
        shell_version: Some("5.2".to_owned()),
        vi_mode_enabled: Some("false".to_owned()),
        os_category: Some("Linux".to_owned()),
        linux_distribution: Some("Debian".to_owned()),
        shell_path: Some("/bin/bash".to_owned()),
        shell_options: Some(vec!["braceexpand".to_owned(), "hashall".to_owned()]),
        shell_plugins: Some(vec!["oh-my-bash".to_owned()]),
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

/// One value of every message the protocol carries, with every nested type
/// populated, so a change to any of them changes the bytes below.
fn every_message() -> Vec<Message> {
    vec![
        Message::Spawn {
            session_id: SessionId::from(1u64),
            options: vec![7, 8, 9],
        },
        Message::SpawnResponse {
            result: Ok(Endpoint { pid: 11, raw: 12 }),
        },
        Message::Detach {
            session_id: SessionId::from(2u64),
            snapshot: snapshot(),
            metadata: metadata(),
        },
        Message::DetachResponse { error: None },
        Message::Attach {
            session_id: SessionId::from(3u64),
            after_sequence: Some(44),
        },
        Message::AttachResponse {
            result: Ok(AttachedSession {
                snapshot: snapshot(),
                buffered_output: vec![1, 2, 3],
                sequence: 55,
                output_was_dropped: true,
                endpoint: Some(Endpoint { pid: 13, raw: 14 }),
                metadata: metadata(),
            }),
        },
        Message::Resize {
            session_id: SessionId::from(4u64),
            columns: 100,
            rows: 40,
        },
        Message::ResizeResponse { error: None },
        Message::Kill {
            session_id: SessionId::from(5u64),
        },
        Message::KillResponse { error: None },
        Message::ChildStatus {
            session_id: SessionId::from(6u64),
        },
        Message::ChildStatusResponse { running: true },
        Message::ListSessions,
        Message::ListSessionsResponse {
            sessions: vec![SessionSummary {
                session_id: SessionId::from(7u64),
                metadata: metadata(),
                detached_for_seconds: 66,
                attached: false,
            }],
        },
        Message::Ping,
        Message::Pong {
            protocol_version: PROTOCOL_VERSION,
        },
        Message::ResponseReceived,
    ]
}

/// The encoding is pinned, because the version in the channel name is the only
/// thing keeping two builds' supervisors apart.
///
/// A supervisor outlives the app that started it. Change a field here without
/// bumping [`PROTOCOL_VERSION`] and a new client shares a channel name with a
/// daemon started weeks ago by a build that no longer exists, and reads its
/// answers as a different struct. That is not a hypothetical: `SessionSummary`
/// gained `attached` between v2 builds, and every `ListSessions` afterwards
/// came back as `invalid u8 while decoding bool`.
///
/// **If this test fails, the encoding changed. Bump `PROTOCOL_VERSION` in
/// `message.rs` and update the bytes below.** Do not update the bytes alone.
#[test]
fn the_wire_encoding_is_what_this_protocol_version_promises() {
    let encoded = bincode::serialize(&every_message()).expect("every message should encode");

    assert_eq!(
        hex::encode(&encoded),
        concat!(
            "110000000000000000000000010000000000000003000000000000000708090100000000",
            "0000000b0000000c00000000000000020000000200000000000000500018000c00000000",
            "0000001b5b481b5b324a68656c6c6f0400000000000000626173680a000000000000002f",
            "686f6d652f7573657205000000000000006275696c640163000000000000000400000000",
            "000000626173680400000000000000757365720400000000000000686f73740106000000",
            "000000005562756e7475000118000000000000002f686f6d652f757365722f2e62617368",
            "5f686973746f7279010a000000000000002f686f6d652f75736572010800000000000000",
            "2f7573722f62696e0104000000000000002f737276010200000000000000766901080000",
            "00000000006c6c3d6c73202d6c010500000000000000673d676974010400000000000000",
            "6d6b63640104000000000000005041544801020000000000000063640102000000000000",
            "006966010300000000000000352e3201050000000000000066616c736501050000000000",
            "00004c696e757801060000000000000044656269616e0109000000000000002f62696e2f",
            "626173680102000000000000000b000000000000006272616365657870616e6407000000",
            "0000000068617368616c6c0101000000000000000a000000000000006f682d6d792d6261",
            "73680300000000040000000300000000000000012c000000000000000500000000000000",
            "500018000c000000000000001b5b481b5b324a68656c6c6f030000000000000001020337",
            "0000000000000001010d0000000e000000000000000400000000000000626173680a0000",
            "00000000002f686f6d652f7573657205000000000000006275696c640163000000000000",
            "000400000000000000626173680400000000000000757365720400000000000000686f73",
            "740106000000000000005562756e7475000118000000000000002f686f6d652f75736572",
            "2f2e626173685f686973746f7279010a000000000000002f686f6d652f75736572010800",
            "0000000000002f7573722f62696e0104000000000000002f737276010200000000000000",
            "76690108000000000000006c6c3d6c73202d6c010500000000000000673d676974010400",
            "0000000000006d6b63640104000000000000005041544801020000000000000063640102",
            "000000000000006966010300000000000000352e3201050000000000000066616c736501",
            "05000000000000004c696e757801060000000000000044656269616e0109000000000000",
            "002f62696e2f626173680102000000000000000b00000000000000627261636565787061",
            "6e64070000000000000068617368616c6c0101000000000000000a000000000000006f68",
            "2d6d792d6261736806000000040000000000000064002800070000000008000000050000",
            "000000000009000000000a00000006000000000000000b000000010c0000000d00000001",
            "0000000000000007000000000000000400000000000000626173680a000000000000002f",
            "686f6d652f7573657205000000000000006275696c640163000000000000000400000000",
            "000000626173680400000000000000757365720400000000000000686f73740106000000",
            "000000005562756e7475000118000000000000002f686f6d652f757365722f2e62617368",
            "5f686973746f7279010a000000000000002f686f6d652f75736572010800000000000000",
            "2f7573722f62696e0104000000000000002f737276010200000000000000766901080000",
            "00000000006c6c3d6c73202d6c010500000000000000673d676974010400000000000000",
            "6d6b63640104000000000000005041544801020000000000000063640102000000000000",
            "006966010300000000000000352e3201050000000000000066616c736501050000000000",
            "00004c696e757801060000000000000044656269616e0109000000000000002f62696e2f",
            "626173680102000000000000000b000000000000006272616365657870616e6407000000",
            "0000000068617368616c6c0101000000000000000a000000000000006f682d6d792d6261",
            "73684200000000000000000e0000000f0000000500000010000000",
        ),
        "the supervisor wire encoding changed; see this test's documentation"
    );
}
