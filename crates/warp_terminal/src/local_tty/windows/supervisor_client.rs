use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use instant::Instant;
use warp_core::SessionId;

use super::daemon_spawn::spawn_detached_supervisor;
use super::pipe_transport::{PipeTransport, existing_pipe_protocol, supervisor_pipe_name_for};
use crate::local_tty::PtyOptions;
use crate::local_tty::screen::ScreenSnapshot;
use crate::local_tty::supervisor::message::{
    AttachedSession, Message, PROTOCOL_VERSION, SessionMetadata, SessionSummary,
};
use crate::local_tty::supervisor::transport::{Endpoint, Transport};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(15);
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// Client for the per-profile Windows terminal supervisor.
pub struct WindowsTerminalSupervisorClient {
    executable: PathBuf,
    existing_pipe: Option<String>,
    // The preservation flag belonging to the currently attached handle for
    // each session. Generations must not share a counter: cleanup of an older
    // pane can lag behind a later attach and must consume only its own detach.
    current_handles: Mutex<HashMap<SessionId, Weak<AtomicBool>>>,
}

impl WindowsTerminalSupervisorClient {
    pub fn new() -> Result<Self> {
        Ok(Self::for_executable(
            std::env::current_exe().context("Failed to locate the Warp executable")?,
        ))
    }

    /// A client for the supervisor belonging to `executable`, which is also what
    /// gets started when no supervisor answers.
    ///
    /// Every build has its own supervisor, so a process other than the Warp
    /// executable itself must name the build whose sessions it wants.
    pub fn for_executable(executable: PathBuf) -> Self {
        Self {
            executable,
            existing_pipe: None,
            current_handles: Mutex::new(HashMap::new()),
        }
    }

    /// Connects only to an already-running, compatible local supervisor; never starts a daemon.
    pub fn for_existing_pipe(pipe: String) -> Result<Self> {
        existing_pipe_protocol(&pipe)?;
        Ok(Self {
            executable: PathBuf::new(),
            existing_pipe: Some(pipe),
            current_handles: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn pipe_name(&self) -> String {
        self.existing_pipe.clone().unwrap_or_else(|| {
            supervisor_pipe_name_for(&warp_core::paths::state_dir(), &self.executable)
        })
    }

    /// Verifies compatibility and that no other window currently owns this session.
    pub fn check_detached(&self, session_id: SessionId) -> Result<()> {
        match self.request(Message::Ping)? {
            Message::Pong { protocol_version }
                if protocol_version
                    == self
                        .existing_pipe
                        .as_deref()
                        .map(existing_pipe_protocol)
                        .transpose()?
                        .unwrap_or(PROTOCOL_VERSION) => {}
            _ => bail!("The old supervisor uses an incompatible protocol"),
        }
        let sessions = self.list_sessions()?;
        let session = sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .context("The old supervisor no longer holds this session")?;
        if session.attached {
            bail!("Another window is attached to this session; detach it there first");
        }
        Ok(())
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
    ///
    /// The supervisor is blind while a client is attached, so a session that
    /// detaches without a snapshot resumes from a blank screen.
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
                self.mark_current_handle_preserved(session_id);
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

    /// Registers the specific PTY handle that owns the current attachment.
    pub fn register_handle(&self, session_id: SessionId) -> Arc<AtomicBool> {
        let preserved = Arc::new(AtomicBool::new(false));
        self.current_handles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(session_id, Arc::downgrade(&preserved));
        preserved
    }

    fn mark_current_handle_preserved(&self, session_id: SessionId) {
        use std::sync::atomic::Ordering;

        let current = self
            .current_handles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&session_id)
            .and_then(Weak::upgrade);
        if let Some(preserved) = current {
            preserved.store(true, Ordering::Release);
        }
    }

    fn request(&self, message: Message) -> Result<Message> {
        let mut transport =
            match PipeTransport::connect_waiting(&self.pipe_name(), CONTROL_TIMEOUT)? {
                Some(transport) => transport,
                None => {
                    if self.existing_pipe.is_some() {
                        bail!("The old supervisor is no longer available");
                    }
                    self.start_supervisor()?;
                    self.connect_after_start()?
                }
            };
        transport.send(&message)?;
        let response = transport.receive_timeout(CONTROL_TIMEOUT)?;
        transport.send(&Message::ResponseReceived)?;
        Ok(response)
    }

    fn start_supervisor(&self) -> Result<()> {
        spawn_detached_supervisor(&self.executable, &["--terminal-session-supervisor"])?;
        Ok(())
    }

    fn connect_after_start(&self) -> Result<PipeTransport> {
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if let Some(transport) = PipeTransport::connect_waiting(&self.pipe_name(), remaining)? {
                return Ok(transport);
            }
            if remaining.is_zero() {
                bail!("Timed out connecting to the terminal supervisor");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

#[cfg(test)]
#[path = "supervisor_client_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "existing_supervisor_tests.rs"]
mod existing_supervisor_tests;
