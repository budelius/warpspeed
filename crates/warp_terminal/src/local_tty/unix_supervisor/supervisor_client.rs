//! The app's side of the Unix supervisor protocol.
//!
//! One connection per request: the supervisor serves a single exchange per
//! accepted socket, which is what keeps its loop coming back round to feed the
//! detached terminals. The connection is dropped as soon as the answer -- and
//! any descriptor lent alongside it -- has arrived.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use instant::Instant;
use warp_core::SessionId;

use super::daemon_spawn::spawn_detached_supervisor;
use super::socket::socket_path;
use super::transport::UnixSocketTransport;
use crate::local_tty::PtyOptions;
use crate::local_tty::screen::ScreenSnapshot;
use crate::local_tty::supervisor::message::{
    AttachedSession, Message, SessionMetadata, SessionSummary,
};
use crate::local_tty::supervisor::transport::{Endpoint, Transport};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(15);
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// Client for the per-user Unix terminal supervisor.
pub struct UnixTerminalSupervisorClient {
    executable: PathBuf,

    /// Sessions handed to the supervisor, which must not be killed when their
    /// pane's PTY handle is dropped.
    preserved_sessions: Mutex<HashSet<SessionId>>,
}

impl UnixTerminalSupervisorClient {
    pub fn new() -> Result<Self> {
        Ok(Self {
            executable: std::env::current_exe().context("Failed to locate the Warp executable")?,
            preserved_sessions: Mutex::new(HashSet::new()),
        })
    }

    pub fn spawn(&self, session_id: SessionId, options: &PtyOptions) -> Result<Endpoint> {
        let options = bincode::serialize(options).context("Failed to encode terminal options")?;
        match self.request(Message::Spawn {
            session_id,
            options,
        })? {
            Message::SpawnResponse { result } => result.map_err(anyhow::Error::msg),
            response => {
                bail!("Terminal supervisor returned an invalid spawn response: {response:?}")
            }
        }
    }

    pub fn attach(&self, session_id: SessionId) -> Result<AttachedSession> {
        match self.request(Message::Attach {
            session_id,
            after_sequence: None,
        })? {
            Message::AttachResponse { result } => result.map_err(anyhow::Error::msg),
            response => {
                bail!("Terminal supervisor returned an invalid attach response: {response:?}")
            }
        }
    }

    /// Hands a session back to the supervisor, along with the screen it was
    /// last showing and what the client knows about it.
    pub fn detach(
        &self,
        session_id: SessionId,
        snapshot: ScreenSnapshot,
        metadata: SessionMetadata,
    ) -> Result<()> {
        match self.request(Message::Detach {
            session_id,
            snapshot,
            metadata,
        })? {
            Message::DetachResponse { error: None } => {
                self.preserved_sessions
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(session_id);
                Ok(())
            }
            Message::DetachResponse { error: Some(error) } => bail!(error),
            response => {
                bail!("Terminal supervisor returned an invalid detach response: {response:?}")
            }
        }
    }

    /// Every session the supervisor is currently holding, attached or not.
    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        match self.request(Message::ListSessions)? {
            Message::ListSessionsResponse { sessions } => Ok(sessions),
            response => {
                bail!("Terminal supervisor returned an invalid session list: {response:?}")
            }
        }
    }

    pub fn resize(&self, session_id: SessionId, columns: u16, rows: u16) -> Result<()> {
        match self.request(Message::Resize {
            session_id,
            columns,
            rows,
        })? {
            Message::ResizeResponse { error: None } => Ok(()),
            Message::ResizeResponse { error: Some(error) } => bail!(error),
            response => {
                bail!("Terminal supervisor returned an invalid resize response: {response:?}")
            }
        }
    }

    pub fn is_running(&self, session_id: SessionId) -> Result<bool> {
        match self.request(Message::ChildStatus { session_id })? {
            Message::ChildStatusResponse { running } => Ok(running),
            response => {
                bail!("Terminal supervisor returned an invalid status response: {response:?}")
            }
        }
    }

    pub fn kill(&self, session_id: SessionId) -> Result<()> {
        match self.request(Message::Kill { session_id })? {
            Message::KillResponse { error: None } => Ok(()),
            Message::KillResponse { error: Some(error) } => bail!(error),
            response => {
                bail!("Terminal supervisor returned an invalid kill response: {response:?}")
            }
        }
    }

    /// Whether `session_id` was detached, clearing the mark as it answers.
    ///
    /// A detached session's PTY handle must not kill it when the pane closes.
    pub fn take_preserved(&self, session_id: SessionId) -> bool {
        self.preserved_sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&session_id)
    }

    fn request(&self, message: Message) -> Result<Message> {
        let path = socket_path();
        let mut transport = match UnixSocketTransport::connect(&path) {
            Ok(transport) => transport,
            Err(_) => {
                self.start_supervisor()?;
                self.connect_after_start(&path)?
            }
        };
        transport.send(&message)?;
        let response = transport.receive_timeout(CONTROL_TIMEOUT)?;
        // Tells the supervisor the answer -- and any descriptor beside it --
        // has been read, so it can drop the connection without discarding an
        // in-flight control message.
        transport.send(&Message::ResponseReceived)?;
        Ok(response)
    }

    fn start_supervisor(&self) -> Result<()> {
        spawn_detached_supervisor(&self.executable, &["--terminal-session-supervisor"])
    }

    fn connect_after_start(&self, path: &std::path::Path) -> Result<UnixSocketTransport> {
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            match UnixSocketTransport::connect(path) {
                Ok(transport) => return Ok(transport),
                Err(error) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(25));
                    let _ = error;
                }
                Err(error) => {
                    return Err(error).context("Timed out connecting to the terminal supervisor");
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "supervisor_client_tests.rs"]
mod tests;
