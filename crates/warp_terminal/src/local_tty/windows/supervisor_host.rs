use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use warp_core::SessionId;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};

use super::attachment_relay::AttachmentRelay;
use super::handle_lending::lend_handle_to_process;
use super::supervised_pty::{SupervisedPty, spawn_supervised_pty_with_options};
use crate::local_tty::PtyOptions;
use crate::local_tty::supervisor::serve::SessionHost;
use crate::local_tty::supervisor::transport::Endpoint;

struct AttachedClient {
    process: HANDLE,
}

impl AttachedClient {
    fn open(process_id: u32) -> Result<Self> {
        let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, process_id) }
            .with_context(|| format!("Failed to track terminal client process {process_id}"))?;
        Ok(Self { process })
    }

    fn is_running(&self) -> bool {
        (unsafe { WaitForSingleObject(self.process, 0) }) == WAIT_TIMEOUT
    }
}

impl Drop for AttachedClient {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.process) };
    }
}

struct HostedSession {
    pty: SupervisedPty,
    attached_client: Option<AttachedClient>,
    attachment_relay: Option<AttachmentRelay>,
    attachment_generation: u64,
}

fn fresh_endpoint(pty: &SupervisedPty, client_pid: u32) -> Result<(Endpoint, AttachmentRelay)> {
    let (relay, client_handle) = AttachmentRelay::start(pty.pipe_handle())?;
    let lent = lend_handle_to_process(client_handle, client_pid);
    let _ = unsafe { CloseHandle(client_handle) };
    let raw = lent?;
    Ok((
        Endpoint {
            pid: pty.pid(),
            raw,
        },
        relay,
    ))
}

/// The Windows process and ConPTY resources owned by the terminal supervisor.
#[derive(Default)]
pub struct WindowsSessionHost {
    sessions: HashMap<SessionId, HostedSession>,
}

impl SessionHost for WindowsSessionHost {
    fn spawn(
        &mut self,
        session_id: SessionId,
        options: PtyOptions,
        client_pid: u32,
    ) -> Result<Endpoint> {
        if self.sessions.contains_key(&session_id) {
            bail!("Terminal session ID is already in use");
        }

        let pty = spawn_supervised_pty_with_options(&options)?;
        let attached_client = AttachedClient::open(client_pid)?;
        let (endpoint, attachment_relay) = fresh_endpoint(&pty, client_pid)?;
        self.sessions.insert(
            session_id,
            HostedSession {
                pty,
                attached_client: Some(attached_client),
                attachment_relay: Some(attachment_relay),
                attachment_generation: 1,
            },
        );
        Ok(endpoint)
    }

    fn detach(&mut self, session_id: SessionId) -> Result<()> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .with_context(|| format!("Terminal session {session_id:?} was not found"))?;
        session.attached_client = None;
        session.attachment_relay = None;
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
                    session.attachment_relay = None;
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
            .read(buffer, 0)
    }

    fn is_running(&mut self, session_id: SessionId) -> bool {
        self.sessions
            .get(&session_id)
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
        let attached_client = AttachedClient::open(client_pid)?;
        let (endpoint, relay) = fresh_endpoint(&session.pty, client_pid)?;
        session.attached_client = Some(attached_client);
        session.attachment_relay = Some(relay);
        session.attachment_generation = session.attachment_generation.saturating_add(1);
        Ok(endpoint)
    }

    fn has_sessions(&self) -> bool {
        self.sessions
            .values()
            .any(|session| session.pty.is_running())
    }
}

#[cfg(test)]
#[path = "supervisor_host_tests.rs"]
mod tests;
