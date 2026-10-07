//! Asks the supervisor that last held a pane's session whether it still does, and ends the
//! session there when the user takes it over.

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use instant::Instant;
use warp_core::SessionId;
use warp_terminal::local_tty::supervisor::message::{Message, PROTOCOL_VERSION};
use warp_terminal::local_tty::supervisor::transport::Transport;

use crate::app_state::SessionHolder;

/// How long one holder answer may take. A holder that is there answers in milliseconds; a
/// missing pipe fails at connect; this only bounds a supervisor that accepted and then stalled.
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
    #[cfg(not(feature = "local_tty"))]
    {
        String::new()
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
    verdict_over(|| connect(channel), session_id)
}

pub(crate) fn end_session(channel: &str, session_id: SessionId) -> Result<()> {
    let mut connected = false;
    let result = end_session_over(
        || {
            let transport = connect(channel);
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

/// `open` is called once per request: a supervisor serves a single request per connection.
pub(crate) fn verdict_over<T: Transport>(
    mut open: impl FnMut() -> Option<T>,
    session_id: SessionId,
) -> HolderVerdict {
    let Some(Message::Pong { protocol_version }) = ask(&mut open, Message::Ping) else {
        return HolderVerdict::Unknown;
    };
    if protocol_version != PROTOCOL_VERSION {
        return HolderVerdict::Unknown;
    }
    let Some(Message::ListSessionsResponse { sessions }) = ask(&mut open, Message::ListSessions)
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

/// `open` is called once per request: a supervisor serves a single request per connection.
pub(crate) fn end_session_over<T: Transport>(
    mut open: impl FnMut() -> Option<T>,
    session_id: SessionId,
) -> Result<()> {
    match ask(&mut open, Message::Kill { session_id }) {
        Some(Message::KillResponse { error: None }) => {}
        Some(Message::KillResponse { error: Some(error) }) => bail!("{error}"),
        _ => bail!("The supervisor did not answer the request to end the session"),
    }
    let deadline = Instant::now() + END_TIMEOUT;
    loop {
        match ask(&mut open, Message::ChildStatus { session_id }) {
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
/// server waits for.
fn ask<T: Transport>(open: &mut impl FnMut() -> Option<T>, message: Message) -> Option<Message> {
    let mut transport = open()?;
    transport.send(&message).ok()?;
    let deadline = Instant::now() + ANSWER_TIMEOUT;
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
fn connect(channel: &str) -> Option<impl Transport> {
    warp_terminal::local_tty::windows::pipe_transport::PipeTransport::connect_waiting(
        channel,
        ANSWER_TIMEOUT,
    )
    .ok()
    .flatten()
}

#[cfg(all(feature = "local_tty", unix))]
fn connect(channel: &str) -> Option<impl Transport> {
    warp_terminal::local_tty::unix_supervisor::transport::UnixSocketTransport::connect(
        std::path::Path::new(channel),
    )
    .ok()
}

#[cfg(not(feature = "local_tty"))]
fn connect(
    _channel: &str,
) -> Option<warp_terminal::local_tty::supervisor::loopback::LoopbackTransport> {
    None
}

#[cfg(test)]
#[path = "session_holder_tests.rs"]
mod tests;
