use std::collections::HashMap;

use super::*;
use crate::local_tty::screen::HeadlessScreen;

/// A host with no processes behind it, so the supervisor's decisions can be
/// tested on their own.
#[derive(Default)]
struct FakeHost {
    /// Output each session will produce on the next read.
    pending_output: HashMap<u64, Vec<u8>>,

    /// Sessions whose process has ended.
    ended: Vec<u64>,

    /// Sessions whose reads fail outright.
    unreadable: Vec<u64>,

    /// Sessions that refuse to lend an endpoint.
    unlendable: Vec<u64>,

    /// Sessions whose client process disappeared without detaching.
    orphaned: Vec<u64>,

    /// Sessions this host was asked to kill.
    killed: Vec<u64>,

    /// Every terminal resize requested from the host.
    resized: Vec<(u64, u16, u16)>,

    /// Every (session, client) pair an endpoint was lent to.
    lent_to: Vec<(u64, u32)>,
}

impl SessionHost for FakeHost {
    fn spawn(
        &mut self,
        session_id: SessionId,
        _options: PtyOptions,
        client_pid: u32,
    ) -> Result<Endpoint> {
        self.lend_endpoint(session_id, client_pid)
    }

    fn detach(&mut self, _session_id: SessionId) -> Result<()> {
        Ok(())
    }

    fn take_orphaned_sessions(&mut self) -> Vec<SessionId> {
        std::mem::take(&mut self.orphaned)
            .into_iter()
            .map(SessionId::from)
            .collect()
    }

    fn read_output(&mut self, session_id: SessionId, buffer: &mut [u8]) -> Result<usize> {
        if self.unreadable.contains(&session_id.as_u64()) {
            anyhow::bail!("this session cannot be read");
        }
        match self.pending_output.remove(&session_id.as_u64()) {
            Some(bytes) => {
                buffer[..bytes.len()].copy_from_slice(&bytes);
                Ok(bytes.len())
            }
            None => Ok(0),
        }
    }

    fn is_running(&mut self, session_id: SessionId) -> bool {
        !self.ended.contains(&session_id.as_u64())
    }

    fn resize(&mut self, session_id: SessionId, columns: u16, rows: u16) -> Result<()> {
        self.resized.push((session_id.as_u64(), columns, rows));
        Ok(())
    }

    fn kill(&mut self, session_id: SessionId) -> Result<()> {
        self.killed.push(session_id.as_u64());
        Ok(())
    }

    fn lend_endpoint(&mut self, session_id: SessionId, client_pid: u32) -> Result<Endpoint> {
        if self.unlendable.contains(&session_id.as_u64()) {
            anyhow::bail!("this session cannot lend its endpoint");
        }
        self.lent_to.push((session_id.as_u64(), client_pid));
        Ok(Endpoint {
            pid: 4242,
            raw: 0x1234,
        })
    }

    fn has_sessions(&self) -> bool {
        true
    }
}

fn listener() -> ChannelEventListener {
    ChannelEventListener::new_for_test()
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

fn snapshot_of(text: &[u8]) -> ScreenSnapshot {
    let mut screen = HeadlessScreen::new(40, 6, listener());
    screen.feed(text);
    screen.snapshot()
}

/// A supervisor holding one detached session, taken over at `at`.
fn supervisor_with_session(id: u64, at: u64) -> Supervisor<FakeHost> {
    let mut supervisor = Supervisor::new(FakeHost::default(), true);
    supervisor.detach(
        SessionId::from(id),
        snapshot_of(b"before"),
        metadata(),
        at,
        listener(),
    );
    supervisor
}

#[test]
fn orphaned_client_process_becomes_reattachable() {
    let mut supervisor = Supervisor::new(
        FakeHost {
            orphaned: vec![42],
            ..Default::default()
        },
        true,
    );

    supervisor.recover_orphaned_sessions(100, &listener());
    let response = supervisor.handle_message(
        &Message::Attach {
            session_id: SessionId::from(42),
            after_sequence: None,
        },
        7,
        101,
        &listener(),
    );

    assert!(matches!(
        response,
        Some(Message::AttachResponse { result: Ok(_) })
    ));
}

/// The reason the supervisor exists: a client that comes back gets the session
/// and something to read it through.
#[test]
fn attaching_lends_the_session_endpoint_to_the_asking_client() {
    let mut supervisor = supervisor_with_session(1, 0);

    let response = supervisor.handle_message(
        &Message::Attach {
            session_id: SessionId::from(1u64),
            after_sequence: None,
        },
        9001,
        0,
        &listener(),
    );

    let Some(Message::AttachResponse {
        result: Ok(attached),
    }) = response
    else {
        panic!("expected a successful attach, got {response:?}");
    };

    assert_eq!(
        attached.endpoint,
        Some(Endpoint {
            pid: 4242,
            raw: 0x1234
        }),
        "the client must be given an endpoint to read the session through"
    );
}

#[test]
fn resizing_while_detached_reflows_the_replay_before_attach() {
    let mut supervisor = supervisor_with_session(1, 0);

    let resized = supervisor.handle_message(
        &Message::Resize {
            session_id: SessionId::from(1u64),
            columns: 90,
            rows: 30,
        },
        9001,
        0,
        &listener(),
    );
    assert_eq!(resized, Some(Message::ResizeResponse { error: None }));

    let response = supervisor.handle_message(
        &Message::Attach {
            session_id: SessionId::from(1u64),
            after_sequence: None,
        },
        9001,
        0,
        &listener(),
    );
    let Some(Message::AttachResponse {
        result: Ok(attached),
    }) = response
    else {
        panic!("expected a successful attach, got {response:?}");
    };
    assert_eq!(
        (attached.snapshot.columns, attached.snapshot.rows),
        (90, 30)
    );
    assert!(attached.output_was_dropped);
    assert_eq!(supervisor.host.resized, [(1, 90, 30)]);
}

/// Over a transport that cannot carry a descriptor -- anything off-machine --
/// the attach still succeeds and the client streams instead.
#[test]
fn attaching_over_a_transport_that_cannot_lend_omits_the_endpoint() {
    let mut supervisor = Supervisor::new(FakeHost::default(), false);
    supervisor.detach(
        SessionId::from(1u64),
        snapshot_of(b"before"),
        metadata(),
        0,
        listener(),
    );

    let response = supervisor.handle_message(
        &Message::Attach {
            session_id: SessionId::from(1u64),
            after_sequence: None,
        },
        9001,
        0,
        &listener(),
    );

    let Some(Message::AttachResponse {
        result: Ok(attached),
    }) = response
    else {
        panic!("expected a successful attach, got {response:?}");
    };

    assert_eq!(
        attached.endpoint, None,
        "a transport that cannot lend must not claim to have lent"
    );
}

/// An attach the table refuses must not lend anything. Handing a descriptor to
/// a client that was told "no" leaks a live PTY to a process that will never
/// close it.
#[test]
fn a_refused_attach_lends_nothing() {
    let mut supervisor = supervisor_with_session(1, 0);

    // The first attach takes the session; the second must be refused.
    supervisor.handle_message(
        &Message::Attach {
            session_id: SessionId::from(1u64),
            after_sequence: None,
        },
        9001,
        0,
        &listener(),
    );
    let response = supervisor.handle_message(
        &Message::Attach {
            session_id: SessionId::from(1u64),
            after_sequence: None,
        },
        9002,
        0,
        &listener(),
    );

    let Some(Message::AttachResponse { result: Err(_) }) = response else {
        panic!("expected the second attach to be refused, got {response:?}");
    };
}

/// If the endpoint cannot be lent, the client has nothing to read from, so the
/// attach has failed -- reporting success would leave a pane wired to nothing.
#[test]
fn an_attach_whose_endpoint_cannot_be_lent_fails() {
    let mut supervisor = Supervisor::new(
        FakeHost {
            unlendable: vec![1],
            ..FakeHost::default()
        },
        true,
    );
    supervisor.detach(
        SessionId::from(1u64),
        snapshot_of(b"before"),
        metadata(),
        0,
        listener(),
    );

    let response = supervisor.handle_message(
        &Message::Attach {
            session_id: SessionId::from(1u64),
            after_sequence: None,
        },
        9001,
        0,
        &listener(),
    );

    let Some(Message::AttachResponse { result: Err(_) }) = response else {
        panic!("an attach that cannot lend an endpoint must fail, got {response:?}");
    };
}

/// Pumping is how a detached session's output reaches the client that comes
/// back for it.
#[test]
fn pumped_output_reaches_a_client_that_reattaches() {
    let mut supervisor = Supervisor::new(
        FakeHost {
            pending_output: HashMap::from([(1, b"while you were away".to_vec())]),
            ..FakeHost::default()
        },
        true,
    );
    supervisor.detach(
        SessionId::from(1u64),
        snapshot_of(b"before"),
        metadata(),
        0,
        listener(),
    );

    supervisor.pump(0, DEFAULT_SESSION_TTL_SECONDS);

    let response = supervisor.handle_message(
        &Message::Attach {
            session_id: SessionId::from(1u64),
            after_sequence: None,
        },
        9001,
        0,
        &listener(),
    );

    let Some(Message::AttachResponse {
        result: Ok(attached),
    }) = response
    else {
        panic!("expected a successful attach, got {response:?}");
    };

    assert_eq!(
        attached.buffered_output, b"while you were away",
        "output collected while detached did not reach the reattaching client"
    );
}

/// A session whose shell has exited is not worth holding: it cannot be attached
/// to usefully and would sit in the drawer forever.
#[test]
fn a_session_whose_process_ended_is_dropped() {
    let mut supervisor = Supervisor::new(
        FakeHost {
            ended: vec![1],
            ..FakeHost::default()
        },
        true,
    );
    supervisor.detach(
        SessionId::from(1u64),
        snapshot_of(b"before"),
        metadata(),
        0,
        listener(),
    );

    let dropped = supervisor.pump(0, DEFAULT_SESSION_TTL_SECONDS);

    assert_eq!(dropped, vec![SessionId::from(1u64)]);
    assert_eq!(supervisor.session_count(), 0);
}

/// A session that cannot be read is unusable, and is dropped for the same
/// reason as one whose process ended.
#[test]
fn a_session_that_cannot_be_read_is_dropped() {
    let mut supervisor = Supervisor::new(
        FakeHost {
            unreadable: vec![1],
            ..FakeHost::default()
        },
        true,
    );
    supervisor.detach(
        SessionId::from(1u64),
        snapshot_of(b"before"),
        metadata(),
        0,
        listener(),
    );

    let dropped = supervisor.pump(0, DEFAULT_SESSION_TTL_SECONDS);

    assert_eq!(dropped, vec![SessionId::from(1u64)]);
}

/// Guards the two tests above: a healthy session must survive a pump, or they
/// would pass against a supervisor that simply drops everything.
#[test]
fn a_healthy_session_survives_a_pump() {
    let mut supervisor = supervisor_with_session(1, 0);

    let dropped = supervisor.pump(0, DEFAULT_SESSION_TTL_SECONDS);

    assert!(dropped.is_empty(), "a healthy session must not be dropped");
    assert_eq!(supervisor.session_count(), 1);
}

/// Expiry has to kill the process too. Dropping the bookkeeping alone would
/// leave an orphaned shell running with nothing tracking it.
#[test]
fn an_expired_session_is_killed_and_not_merely_forgotten() {
    let mut supervisor = supervisor_with_session(1, 0);

    let dropped = supervisor.pump(10_000, 300);

    assert_eq!(dropped, vec![SessionId::from(1u64)]);
    assert_eq!(supervisor.session_count(), 0);
}

/// Listing is what fills the drawer, and it has to describe the session with
/// what the client told us when it let go.
#[test]
fn listing_describes_held_sessions() {
    let mut supervisor = supervisor_with_session(1, 100);

    let response = supervisor.handle_message(&Message::ListSessions, 0, 160, &listener());

    let Some(Message::ListSessionsResponse { sessions }) = response else {
        panic!("expected a session list, got {response:?}");
    };

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].metadata.shell, "bash");
    assert_eq!(sessions[0].detached_for_seconds, 60);
}

/// A detach over the wire has to land in the table, or a client that quits
/// would lose the session it just handed over.
#[test]
fn a_detach_message_takes_over_the_session() {
    let mut supervisor = Supervisor::new(FakeHost::default(), true);

    let response = supervisor.handle_message(
        &Message::Detach {
            session_id: SessionId::from(5u64),
            snapshot: snapshot_of(b"handed over"),
            metadata: metadata(),
        },
        9001,
        0,
        &listener(),
    );

    assert_eq!(response, Some(Message::DetachResponse { error: None }));
    assert_eq!(supervisor.session_count(), 1);
}

/// A window that restored a pane for a held session but never opened it still lists that session
/// when it closes. Its detach carries an empty screen and no metadata, and accepting it would leave
/// the next pane to adopt the shell with nothing to replay and no idea the shell sits at a prompt.
#[test]
fn a_detach_of_a_session_nobody_attached_keeps_what_the_supervisor_holds() {
    let mut supervisor = supervisor_with_session(1, 0);

    let response = supervisor.handle_message(
        &Message::Detach {
            session_id: SessionId::from(1u64),
            snapshot: snapshot_of(b""),
            metadata: SessionMetadata::default(),
        },
        9001,
        10,
        &listener(),
    );

    assert_eq!(response, Some(Message::DetachResponse { error: None }));
    let Some(Message::AttachResponse {
        result: Ok(attached),
    }) = supervisor.handle_message(
        &Message::Attach {
            session_id: SessionId::from(1u64),
            after_sequence: None,
        },
        7,
        11,
        &listener(),
    )
    else {
        panic!("the held session should still attach");
    };
    assert_eq!(attached.metadata, metadata());
    assert_eq!(attached.snapshot, snapshot_of(b"before"));
}
