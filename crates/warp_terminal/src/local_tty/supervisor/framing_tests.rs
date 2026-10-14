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
        input_editor_active: true,
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
                shell_pid: 42,
                session_uuid_hex: "a1b2c3".to_owned(),
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
/// `message.rs` and update `wire_v7.hex`.** Do not update the golden bytes alone.
#[test]
fn the_wire_encoding_is_what_this_protocol_version_promises() {
    let encoded = bincode::serialize(&every_message()).expect("every message should encode");

    assert_eq!(
        hex::encode(&encoded),
        include_str!("wire_v7.hex").trim(),
        "the supervisor wire encoding changed; see this test's documentation"
    );
}
