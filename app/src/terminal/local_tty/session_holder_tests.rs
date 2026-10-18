use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

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
        shell_pid: 42,
        session_uuid_hex: "a1b2c3".to_owned(),
    }
}

/// Answers like a supervisor: one request per connection, then the `ResponseReceived`
/// acknowledgement, then the next connection. `Ping` is answered with the given version,
/// `ListSessions` with `sessions`, `Kill` with `kill_error`, and `ChildStatus` with `running`
/// decreasing to false after `running_polls` answers. Returns every request it saw.
fn fake_holder(
    connections: mpsc::Receiver<LoopbackTransport>,
    version: u32,
    sessions: Vec<SessionSummary>,
    kill_error: Option<String>,
    mut running_polls: usize,
) -> thread::JoinHandle<Vec<Message>> {
    thread::spawn(move || {
        let mut seen = Vec::new();
        for mut side in connections {
            let Some(message) = receive_blocking(&mut side) else {
                continue;
            };
            let reply = match &message {
                Message::Ping => Message::Pong {
                    protocol_version: version,
                },
                Message::ListSessions => Message::ListSessionsResponse {
                    sessions: sessions.clone(),
                },
                Message::Kill { .. } => Message::KillResponse {
                    error: kill_error.clone(),
                },
                Message::ChildStatus { .. } => {
                    let running = running_polls > 0;
                    running_polls = running_polls.saturating_sub(1);
                    Message::ChildStatusResponse { running }
                }
                _ => Message::KillResponse {
                    error: Some("unexpected".into()),
                },
            };
            seen.push(message);
            side.send(&reply).unwrap();
            let acknowledgement = receive_blocking(&mut side);
            assert!(matches!(acknowledgement, Some(Message::ResponseReceived)));
        }
        seen
    })
}

fn receive_blocking(side: &mut LoopbackTransport) -> Option<Message> {
    for _ in 0..400 {
        if let Ok(Some(message)) = side.receive() {
            return Some(message);
        }
        thread::sleep(Duration::from_millis(5));
    }
    None
}

/// An opener that hands the server side of a fresh pair to the fake holder on every call.
fn opener(
    connections: mpsc::Sender<LoopbackTransport>,
) -> impl FnMut(Duration) -> Option<LoopbackTransport> {
    move |_| {
        let (client, server) = LoopbackTransport::pair();
        connections.send(server).unwrap();
        Some(client)
    }
}

#[test]
fn a_detached_session_the_holder_lists_is_held_detached() {
    let (sender, receiver) = mpsc::channel();
    let holder = fake_holder(
        receiver,
        PROTOCOL_VERSION,
        vec![summary(42, false)],
        None,
        0,
    );
    assert_eq!(
        verdict_over(opener(sender), SessionId::from(42u64)),
        HolderVerdict::HeldDetached
    );
    holder.join().unwrap();
}

#[cfg(windows)]
#[test]
fn v6_sessions_are_not_mistaken_for_missing_sessions() {
    let (sender, receiver) = mpsc::channel();
    let holder = fake_holder(receiver, 6, vec![summary(42, false)], None, 0);
    assert_eq!(
        verdict_over(opener(sender), SessionId::from(42)),
        HolderVerdict::HeldDetached
    );
    holder.join().unwrap();
}

#[test]
fn an_attached_session_is_held_attached() {
    let (sender, receiver) = mpsc::channel();
    let holder = fake_holder(receiver, PROTOCOL_VERSION, vec![summary(42, true)], None, 0);
    assert_eq!(
        verdict_over(opener(sender), SessionId::from(42u64)),
        HolderVerdict::HeldAttached
    );
    holder.join().unwrap();
}

#[test]
fn a_session_the_holder_does_not_list_is_unknown() {
    let (sender, receiver) = mpsc::channel();
    let holder = fake_holder(receiver, PROTOCOL_VERSION, vec![summary(1, false)], None, 0);
    assert_eq!(
        verdict_over(opener(sender), SessionId::from(42u64)),
        HolderVerdict::Unknown
    );
    holder.join().unwrap();
}

#[test]
fn a_holder_on_another_protocol_version_is_unknown_and_never_asked_for_sessions() {
    let (sender, receiver) = mpsc::channel();
    let holder = fake_holder(
        receiver,
        PROTOCOL_VERSION + 1,
        vec![summary(42, false)],
        None,
        0,
    );
    assert_eq!(
        verdict_over(opener(sender), SessionId::from(42u64)),
        HolderVerdict::Unknown
    );
    let seen = holder.join().unwrap();
    assert!(!seen.iter().any(|m| matches!(m, Message::ListSessions)));
}

#[test]
fn every_request_is_made_on_its_own_connection() {
    let (sender, receiver) = mpsc::channel();
    let counted = Arc::new(AtomicUsize::new(0));
    let holder = fake_holder(
        receiver,
        PROTOCOL_VERSION,
        vec![summary(42, false)],
        None,
        0,
    );
    let mut open = opener(sender);
    let open_counted = {
        let counted = counted.clone();
        move |timeout| {
            counted.fetch_add(1, Ordering::SeqCst);
            open(timeout)
        }
    };
    verdict_over(open_counted, SessionId::from(42u64));
    let seen = holder.join().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(counted.load(Ordering::SeqCst), seen.len());
}

/// The verdict is asked on the main thread, so a holder that accepts and then never answers must
/// cost the window at most the verdict's one shared budget, not a timeout per request.
#[test]
fn a_holder_that_accepts_and_never_answers_is_unknown_within_the_verdict_budget() {
    let (sender, receiver) = mpsc::channel();
    let silent_holder = thread::spawn(move || receiver.into_iter().collect::<Vec<_>>());
    let started = Instant::now();
    let verdict = verdict_over(opener(sender), SessionId::from(42u64));
    let elapsed = started.elapsed();
    assert_eq!(verdict, HolderVerdict::Unknown);
    assert!(
        elapsed < Duration::from_secs(2),
        "a stalled holder held the verdict up for {elapsed:?}"
    );
    silent_holder.join().unwrap();
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
    let (sender, receiver) = mpsc::channel();
    let holder = fake_holder(
        receiver,
        PROTOCOL_VERSION,
        vec![summary(42, false)],
        None,
        2,
    );
    end_session_over(opener(sender), SessionId::from(42u64)).unwrap();
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
    let (sender, receiver) = mpsc::channel();
    let holder = fake_holder(
        receiver,
        PROTOCOL_VERSION,
        vec![],
        Some("no such session".into()),
        0,
    );
    let error = end_session_over(opener(sender), SessionId::from(42u64)).unwrap_err();
    assert!(error.to_string().contains("no such session"));
    holder.join().unwrap();
}

#[test]
fn a_take_over_ends_the_session_only_while_it_is_still_held_detached() {
    assert_eq!(
        take_over_step(HolderVerdict::HeldDetached, false).unwrap(),
        TakeOverStep::EndAgentAndSession
    );
    assert_eq!(
        take_over_step(HolderVerdict::HeldAttached, true)
            .unwrap_err()
            .to_string(),
        "A window is attached to that session now"
    );
    assert_eq!(
        take_over_step(HolderVerdict::Unknown, true).unwrap(),
        TakeOverStep::EndAgentOnly
    );
    assert_eq!(
        take_over_step(HolderVerdict::Unknown, false)
            .unwrap_err()
            .to_string(),
        "The session is no longer held"
    );
}
