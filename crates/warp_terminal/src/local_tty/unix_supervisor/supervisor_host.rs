//! The processes and descriptors the Unix terminal supervisor owns.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use warp_core::SessionId;

use super::client_process::ClientProcess;
use super::supervised_pty::SupervisedPty;
use crate::local_tty::PtyOptions;
use crate::local_tty::supervisor::serve::SessionHost;
use crate::local_tty::supervisor::transport::Endpoint;

struct HostedSession {
    pty: SupervisedPty,
    attached_client: Option<ClientProcess>,
}

/// The Unix side of [`SessionHost`].
///
/// The endpoint it lends is the PTY leader descriptor as numbered *here*; the
/// transport is what turns that into a descriptor the client owns. See
/// `transport.rs`.
#[derive(Default)]
pub struct UnixSessionHost {
    sessions: HashMap<SessionId, HostedSession>,
}

impl SessionHost for UnixSessionHost {
    fn spawn(
        &mut self,
        session_id: SessionId,
        options: PtyOptions,
        client_pid: u32,
    ) -> Result<Endpoint> {
        if self.sessions.contains_key(&session_id) {
            bail!("Terminal session ID is already in use");
        }

        let pty = SupervisedPty::spawn(options)?;
        let endpoint = Endpoint {
            pid: pty.pid(),
            raw: pty.leader_fd() as i64,
        };
        self.sessions.insert(
            session_id,
            HostedSession {
                pty,
                attached_client: Some(ClientProcess::new(client_pid)),
            },
        );
        Ok(endpoint)
    }

    fn detach(&mut self, session_id: SessionId) -> Result<()> {
        self.sessions
            .get_mut(&session_id)
            .with_context(|| format!("Terminal session {session_id:?} was not found"))?
            .attached_client = None;
        Ok(())
    }

    fn take_orphaned_sessions(&mut self) -> Vec<SessionId> {
        self.sessions
            .iter_mut()
            .filter_map(|(session_id, session)| {
                let orphaned = session
                    .attached_client
                    .as_ref()
                    .is_some_and(|client| !client.is_running());
                if orphaned {
                    session.attached_client = None;
                }
                orphaned.then_some(*session_id)
            })
            .collect()
    }

    fn read_output(&mut self, session_id: SessionId, buffer: &mut [u8]) -> Result<usize> {
        self.sessions
            .get_mut(&session_id)
            .with_context(|| format!("Terminal session {session_id:?} was not found"))?
            .pty
            .read(buffer)
    }

    fn is_running(&mut self, session_id: SessionId) -> bool {
        self.sessions
            .get_mut(&session_id)
            .is_some_and(|session| session.pty.is_running())
    }

    fn resize(&mut self, session_id: SessionId, columns: u16, rows: u16) -> Result<()> {
        self.sessions
            .get_mut(&session_id)
            .with_context(|| format!("Terminal session {session_id:?} was not found"))?
            .pty
            .resize(columns, rows)
    }

    fn kill(&mut self, session_id: SessionId) -> Result<()> {
        let Some(mut session) = self.sessions.remove(&session_id) else {
            return Ok(());
        };
        session.pty.kill()
    }

    fn lend_endpoint(&mut self, session_id: SessionId, client_pid: u32) -> Result<Endpoint> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .with_context(|| format!("Terminal session {session_id:?} was not found"))?;
        let endpoint = Endpoint {
            pid: session.pty.pid(),
            raw: session.pty.leader_fd() as i64,
        };
        session.attached_client = Some(ClientProcess::new(client_pid));
        Ok(endpoint)
    }

    /// Whether any session is still held, rather than whether any shell is
    /// still alive.
    ///
    /// Reaping a child needs `&mut`, which this signature does not have. The
    /// difference only delays the daemon's idle shutdown, and only until
    /// whoever holds the dead session lets go of it: a detached one is dropped
    /// by the next `pump`, and an attached one by the pane that notices its
    /// shell exited.
    fn has_sessions(&self) -> bool {
        !self.sessions.is_empty()
    }
}

#[cfg(test)]
#[path = "supervisor_host_tests.rs"]
mod tests;
