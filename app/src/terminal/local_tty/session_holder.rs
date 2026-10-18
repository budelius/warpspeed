//! Asks the supervisor that last held a pane's session whether it still does, and ends the
//! session there when the user takes it over.

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use instant::Instant;
use warp_core::SessionId;
use warp_terminal::local_tty::supervisor::message::Message;
#[cfg(not(windows))]
use warp_terminal::local_tty::supervisor::message::PROTOCOL_VERSION;
use warp_terminal::local_tty::supervisor::transport::Transport;

use crate::app_state::SessionHolder;

/// How long a whole verdict may take, connects and answers of both its requests included. It is
/// asked on the main thread while a restored pane starts, so a holder that accepted and then
/// stalled must not hold the window up. A holder that is there answers in milliseconds; a missing
/// pipe fails at connect.
const VERDICT_TIMEOUT: Duration = Duration::from_secs(1);
/// How long one request may take while a session is being ended, which runs off the main thread.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(2);
/// How long to wait for a killed session's shell to go.
const END_TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HolderVerdict {
    /// Unreachable, another protocol, or the session is not there: restore as today.
    Unknown,
    /// Held with no window on it.
    HeldDetached,
    /// Held and a window is attached to it.
    HeldAttached,
}

/// What a take-over has to end, by what the holder says right before anything is ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TakeOverStep {
    EndAgentAndSession,
    /// The holder no longer has the session, but the agent is still running somewhere.
    EndAgentOnly,
}

/// The holder is asked again at take-over rather than trusted from the restore: a window may have
/// attached since, and a supervisor ends a session whether or not a window is attached to it.
pub(crate) fn take_over_step(
    verdict: HolderVerdict,
    agent_registered: bool,
) -> Result<TakeOverStep> {
    match verdict {
        HolderVerdict::HeldDetached => Ok(TakeOverStep::EndAgentAndSession),
        HolderVerdict::HeldAttached => bail!("A window is attached to that session now"),
        HolderVerdict::Unknown if agent_registered => Ok(TakeOverStep::EndAgentOnly),
        HolderVerdict::Unknown => bail!("The session is no longer held"),
    }
}

pub(crate) fn current_channel() -> String {
    #[cfg(all(feature = "local_tty", windows))]
    {
        warp_terminal::local_tty::windows::pipe_transport::supervisor_pipe_name()
    }
    #[cfg(all(feature = "local_tty", unix))]
    {
        warp_terminal::local_tty::unix_supervisor::socket::socket_path()
            .to_string_lossy()
            .into_owned()
    }
}

pub(crate) fn current_holder() -> SessionHolder {
    SessionHolder {
        channel: current_channel(),
        version: crate::remote_control::build_identity(),
        pid: std::process::id(),
    }
}

pub(crate) fn verdict(channel: &str, session_id: SessionId) -> HolderVerdict {
    verdict_over(|timeout| connect(channel, timeout), session_id)
}

pub(crate) fn end_session(channel: &str, session_id: SessionId) -> Result<()> {
    let mut connected = false;
    let result = end_session_over(
        |timeout| {
            let transport = connect(channel, timeout);
            connected |= transport.is_some();
            transport
        },
        session_id,
    );
    if connected {
        result
    } else {
        result.with_context(|| format!("The supervisor on {channel} is not answering"))
    }
}

/// `open` is called once per request, with how long it may wait to connect: a supervisor serves a
/// single request per connection.
pub(crate) fn verdict_over<T: Transport>(
    mut open: impl FnMut(Duration) -> Option<T>,
    session_id: SessionId,
) -> HolderVerdict {
    let deadline = Instant::now() + VERDICT_TIMEOUT;
    let Some(Message::Pong { protocol_version }) = ask(&mut open, Message::Ping, deadline) else {
        return HolderVerdict::Unknown;
    };
    #[cfg(windows)]
    let compatible =
        warp_terminal::local_tty::supervisor::compatibility::supports_protocol(protocol_version);
    #[cfg(not(windows))]
    let compatible = protocol_version == PROTOCOL_VERSION;
    if !compatible {
        return HolderVerdict::Unknown;
    }
    let Some(Message::ListSessionsResponse { sessions }) =
        ask(&mut open, Message::ListSessions, deadline)
    else {
        return HolderVerdict::Unknown;
    };
    match sessions
        .iter()
        .find(|session| session.session_id == session_id)
    {
        Some(session) if session.attached => HolderVerdict::HeldAttached,
        Some(_) => HolderVerdict::HeldDetached,
        None => HolderVerdict::Unknown,
    }
}

/// `open` is called once per request, with how long it may wait to connect: a supervisor serves a
/// single request per connection.
pub(crate) fn end_session_over<T: Transport>(
    mut open: impl FnMut(Duration) -> Option<T>,
    session_id: SessionId,
) -> Result<()> {
    match ask(
        &mut open,
        Message::Kill { session_id },
        Instant::now() + ANSWER_TIMEOUT,
    ) {
        Some(Message::KillResponse { error: None }) => {}
        Some(Message::KillResponse { error: Some(error) }) => bail!("{error}"),
        _ => bail!("The supervisor did not answer the request to end the session"),
    }
    let deadline = Instant::now() + END_TIMEOUT;
    loop {
        match ask(
            &mut open,
            Message::ChildStatus { session_id },
            Instant::now() + ANSWER_TIMEOUT,
        ) {
            Some(Message::ChildStatusResponse { running: false }) => return Ok(()),
            Some(Message::ChildStatusResponse { running: true }) => {}
            _ => bail!("The supervisor stopped answering while the session was ending"),
        }
        if Instant::now() >= deadline {
            bail!("The session's shell did not end in time");
        }
        std::thread::sleep(POLL);
    }
}

/// One request on a fresh connection, its answer, and the acknowledgement the named-pipe
/// server waits for, all by `deadline`.
fn ask<T: Transport>(
    open: &mut impl FnMut(Duration) -> Option<T>,
    message: Message,
    deadline: Instant,
) -> Option<Message> {
    let connect_timeout = deadline.checked_duration_since(Instant::now())?;
    let mut transport = open(connect_timeout)?;
    transport.send(&message).ok()?;
    let answer = loop {
        match transport.receive() {
            Ok(Some(answer)) => break Some(answer),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) | Err(_) => break None,
        }
    };
    let _ = transport.send(&Message::ResponseReceived);
    answer
}

#[cfg(all(feature = "local_tty", windows))]
fn connect(channel: &str, timeout: Duration) -> Option<impl Transport> {
    warp_terminal::local_tty::windows::pipe_transport::PipeTransport::connect_waiting(
        channel, timeout,
    )
    .ok()
    .flatten()
}

/// A socket connect does not wait for a busy server the way a pipe connect does, so it has no use
/// for the timeout.
#[cfg(all(feature = "local_tty", unix))]
#[allow(unused_variables)]
fn connect(channel: &str, timeout: Duration) -> Option<impl Transport> {
    warp_terminal::local_tty::unix_supervisor::transport::UnixSocketTransport::connect(
        std::path::Path::new(channel),
    )
    .ok()
}

#[cfg(test)]
#[path = "session_holder_tests.rs"]
mod tests;
