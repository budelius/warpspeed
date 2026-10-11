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
        input_editor_active: false,
        bootstrap: None,
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

#[test]
fn each_build_gets_its_own_supervisor_pipe() {
    let state = Path::new(r"C:\Users\someone\AppData\Local\warp\WarpSpeed\data");
    let old_build = Path::new(r"C:\WarpSpeed\versions\20261002-a\warpspeed.exe");
    let new_build = Path::new(r"C:\WarpSpeed\versions\20261003-b\warpspeed.exe");

    assert_ne!(
        supervisor_pipe_name_for(state, old_build),
        supervisor_pipe_name_for(state, new_build)
    );
    assert_eq!(
        supervisor_pipe_name_for(state, new_build),
        supervisor_pipe_name_for(state, new_build),
        "a window and the supervisor it started must agree on the name"
    );
}

#[test]
fn the_supervisor_pipe_ignores_the_executable_path_case() {
    let state = Path::new(r"C:\data");

    assert_eq!(
        supervisor_pipe_name_for(state, Path::new(r"C:\WarpSpeed\warpspeed.exe")),
        supervisor_pipe_name_for(state, Path::new(r"c:\warpspeed\WARPSPEED.EXE"))
    );
}

#[test]
fn separate_state_directories_keep_separate_supervisors() {
    let executable = Path::new(r"C:\WarpSpeed\warpspeed.exe");

    assert_ne!(
        supervisor_pipe_name_for(Path::new(r"C:\one"), executable),
        supervisor_pipe_name_for(Path::new(r"C:\two"), executable)
    );
}

/// Two supervisors must not end up serving one name.
///
/// `CreateNamedPipeW` is happy to add a second *server* instance to an existing
/// name, and a client is then handed whichever instance is free. Each supervisor
/// would hold sessions the other never saw: the picker would list half of them,
/// and an attach landing on the wrong one would report the session missing and
/// open a fresh shell. Observed in the wild as three daemons against two names,
/// two of them started a second apart.
#[test]
fn a_second_supervisor_is_refused_the_same_pipe_name() {
    let name = test_pipe_name("exclusive");
    let _first = PipeListener::bind(name.clone()).expect("the first bind must succeed");

    let second = PipeListener::bind(name.clone());

    let Err(error) = second else {
        panic!("a second supervisor must not take the name");
    };
    assert!(
        error.to_string().contains("already listening"),
        "error was {error}"
    );
}

/// The refusal has to be about *another* process holding the name, not about
/// exclusivity as such: a listener creates a fresh instance after every accept,
/// and asking for a first instance there would refuse its own name.
#[test]
fn a_listener_can_still_serve_client_after_client() {
    let name = test_pipe_name("sequential");
    let mut listener = PipeListener::bind(name.clone()).expect("bind must succeed");

    for _ in 0..3 {
        let mut client = PipeTransport::connect(&name).expect("connect must succeed");
        let mut supervisor = listener.accept().expect("accept must succeed");
        client.send(&Message::Ping).expect("send must succeed");
        assert_eq!(
            supervisor.receive().expect("receive must succeed"),
            Some(Message::Ping)
        );
    }
}

#[test]
fn waiting_for_an_absent_supervisor_reports_that_none_is_listening() {
    let name = test_pipe_name("absent");

    let transport = PipeTransport::connect_waiting(&name, std::time::Duration::from_secs(5))
        .expect("an absent supervisor is not an error");

    assert!(transport.is_none());
}

/// A supervisor that is busy with another client is still running. Mistaking it for an absent
/// one makes every waiting client start its own supervisor, which is how hundreds of losing
/// `--terminal-session-supervisor` processes ended up cycling against one live daemon.
#[test]
fn a_busy_supervisor_is_waited_for_rather_than_reported_absent() {
    let name = test_pipe_name("busy");
    let mut listener = PipeListener::bind(name.clone()).expect("bind must succeed");
    let _occupant = PipeTransport::connect(&name).expect("the first client must connect");
    assert!(
        PipeTransport::connect(&name).is_err(),
        "with its only instance taken, the pipe must be busy for this test to mean anything"
    );

    let waiter = std::thread::spawn(move || {
        PipeTransport::connect_waiting(&name, std::time::Duration::from_secs(5))
            .expect("a busy supervisor must be waited for")
            .is_some()
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    // Accepting the occupant stands up the next instance, which frees the waiter.
    let _occupant_server_end = listener.accept().expect("accept must succeed");

    assert!(waiter.join().expect("the waiting client must not panic"));
}

/// A name whose supervisor has gone is free to take, so a restart is not locked
/// out by its own predecessor.
#[test]
fn a_name_released_by_a_dead_supervisor_can_be_taken_again() {
    let name = test_pipe_name("released");
    drop(PipeListener::bind(name.clone()).expect("the first bind must succeed"));

    let second = PipeListener::bind(name.clone());

    assert!(second.is_ok(), "a released name must be bindable again");
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
