use std::thread;

use warp_core::SessionId;
use warp_terminal::local_tty::supervisor::loopback::LoopbackTransport;
use warp_terminal::local_tty::supervisor::message::{
    Message, PROTOCOL_VERSION, SessionMetadata, SessionSummary,
};
use warp_terminal::local_tty::supervisor::transport::Transport;

use super::*;

fn summary(id: u64, attached: bool) -> SessionSummary {
    SessionSummary {
        session_id: SessionId::from(id),
        metadata: SessionMetadata {
            shell: "bash".to_owned(),
            working_directory: r"T:\projects".to_owned(),
            title: String::new(),
            ..Default::default()
        },
        detached_for_seconds: 3,
        attached,
    }
}

/// Answers like a supervisor: `Ping` with the given version, `ListSessions` with `sessions`,
/// `Kill` with `kill_error`, and `ChildStatus` with `running` decreasing to false after
/// `running_polls` answers.
fn fake_holder(
    mut side: LoopbackTransport,
    version: u32,
    sessions: Vec<SessionSummary>,
    kill_error: Option<String>,
    mut running_polls: usize,
) -> thread::JoinHandle<Vec<Message>> {
    thread::spawn(move || {
        let mut seen = Vec::new();
        let mut idle_polls = 0;
        loop {
            match side.receive() {
                Ok(Some(message)) => {
                    idle_polls = 0;
                    let reply = match &message {
                        Message::Ping => Some(Message::Pong {
                            protocol_version: version,
                        }),
                        Message::ListSessions => Some(Message::ListSessionsResponse {
                            sessions: sessions.clone(),
                        }),
                        Message::Kill { .. } => Some(Message::KillResponse {
                            error: kill_error.clone(),
                        }),
                        Message::ChildStatus { .. } => {
                            let running = running_polls > 0;
                            running_polls = running_polls.saturating_sub(1);
                            Some(Message::ChildStatusResponse { running })
                        }
                        Message::ResponseReceived => None,
                        _ => Some(Message::KillResponse {
                            error: Some("unexpected".into()),
                        }),
                    };
                    seen.push(message);
                    if let Some(reply) = reply {
                        side.send(&reply).unwrap();
                    }
                }
                Ok(None) => {
                    idle_polls += 1;
                    if idle_polls > 100 {
                        return seen;
                    }
                    thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(_) => return seen,
            }
        }
    })
}

#[test]
fn a_detached_session_the_holder_lists_is_held_detached() {
    let (mut client, server) = LoopbackTransport::pair();
    let holder = fake_holder(server, PROTOCOL_VERSION, vec![summary(42, false)], None, 0);
    assert_eq!(
        verdict_over(&mut client, SessionId::from(42u64)),
        HolderVerdict::HeldDetached
    );
    drop(client);
    holder.join().unwrap();
}

#[test]
fn an_attached_session_is_held_attached() {
    let (mut client, server) = LoopbackTransport::pair();
    let holder = fake_holder(server, PROTOCOL_VERSION, vec![summary(42, true)], None, 0);
    assert_eq!(
        verdict_over(&mut client, SessionId::from(42u64)),
        HolderVerdict::HeldAttached
    );
    drop(client);
    holder.join().unwrap();
}

#[test]
fn a_session_the_holder_does_not_list_is_unknown() {
    let (mut client, server) = LoopbackTransport::pair();
    let holder = fake_holder(server, PROTOCOL_VERSION, vec![summary(1, false)], None, 0);
    assert_eq!(
        verdict_over(&mut client, SessionId::from(42u64)),
        HolderVerdict::Unknown
    );
    drop(client);
    holder.join().unwrap();
}

#[test]
fn a_holder_on_another_protocol_version_is_unknown_and_never_asked_for_sessions() {
    let (mut client, server) = LoopbackTransport::pair();
    let holder = fake_holder(
        server,
        PROTOCOL_VERSION + 1,
        vec![summary(42, false)],
        None,
        0,
    );
    assert_eq!(
        verdict_over(&mut client, SessionId::from(42u64)),
        HolderVerdict::Unknown
    );
    drop(client);
    let seen = holder.join().unwrap();
    assert!(!seen.iter().any(|m| matches!(m, Message::ListSessions)));
}

#[cfg(unix)]
#[test]
fn an_unreachable_channel_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let channel = dir
        .path()
        .join("missing.sock")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        verdict(&channel, SessionId::from(42u64)),
        HolderVerdict::Unknown
    );
}

#[cfg(windows)]
#[test]
fn an_unreachable_channel_is_unknown() {
    let channel = format!(
        r"\\.\pipe\warp-terminal-supervisor-test-missing-{}",
        std::process::id()
    );
    assert_eq!(
        verdict(&channel, SessionId::from(42u64)),
        HolderVerdict::Unknown
    );
}

#[test]
fn ending_a_session_kills_it_and_waits_until_it_stops_running() {
    let (mut client, server) = LoopbackTransport::pair();
    let holder = fake_holder(server, PROTOCOL_VERSION, vec![summary(42, false)], None, 2);
    end_session_over(&mut client, SessionId::from(42u64)).unwrap();
    drop(client);
    let seen = holder.join().unwrap();
    assert!(
        seen.iter()
            .any(|m| matches!(m, Message::Kill { session_id } if session_id.as_u64() == 42))
    );
    assert_eq!(
        seen.iter()
            .filter(|m| matches!(m, Message::ChildStatus { .. }))
            .count(),
        3
    );
}

#[test]
fn a_kill_the_holder_refuses_is_an_error() {
    let (mut client, server) = LoopbackTransport::pair();
    let holder = fake_holder(
        server,
        PROTOCOL_VERSION,
        vec![],
        Some("no such session".into()),
        0,
    );
    let error = end_session_over(&mut client, SessionId::from(42u64)).unwrap_err();
    assert!(error.to_string().contains("no such session"));
    drop(client);
    holder.join().unwrap();
}
