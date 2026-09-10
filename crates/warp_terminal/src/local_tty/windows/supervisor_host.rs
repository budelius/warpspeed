use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use warp_core::SessionId;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};

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

    /// Whether this session's pipe has ever been lent to a client.
    ///
    /// It can only be lent once, ever. See [`WindowsSessionHost::lend_endpoint`].
    pipe_was_lent: bool,
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
        let endpoint = Endpoint {
            pid: pty.pid(),
            raw: lend_handle_to_process(pty.pipe_handle(), client_pid)?,
        };
        let attached_client = AttachedClient::open(client_pid)?;
        self.sessions.insert(
            session_id,
            HostedSession {
                pty,
                attached_client: Some(attached_client),
                pipe_was_lent: true,
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

    /// Refuses to lend a pipe a second time, because Windows will not let a
    /// second client read it.
    ///
    /// Lending duplicates a handle, and a duplicate points at the same kernel
    /// file object. The first client registers that object for asynchronous
    /// reads, which binds it to that client's completion port -- permanently:
    /// "a handle can be associated with only one I/O completion port, and after
    /// making the association, the handle remains associated with that I/O
    /// completion port until it is closed". The binding is on the object, so it
    /// outlives the handle, the completion port, and the process.
    ///
    /// A second client therefore cannot read what it is lent. It used to find
    /// that out by registering the handle and having the kernel refuse with
    /// `ERROR_INVALID_PARAMETER`, which arrived as a panic on the `PTY reader`
    /// thread and a tab that closed itself -- while the supervisor went on
    /// believing the session was in use, so nothing could attach it afterwards
    /// either. `local_tty::windows::iocp_reuse_tests` pins the underlying
    /// behaviour down.
    ///
    /// Refusing here instead leaves the session exactly where it was: the serve
    /// loop marks it detached again and the client reports a failed attach, so
    /// the shell keeps running and stays available to a build that can take it.
    ///
    /// Making attach work needs the supervisor to stop lending this pipe twice
    /// -- by handing over a fresh pipe and relaying between the two, or by
    /// giving the client something it can read without a completion port.
    fn lend_endpoint(&mut self, session_id: SessionId, client_pid: u32) -> Result<Endpoint> {
        let session = self
            .sessions
            .get_mut(&session_id)
            .with_context(|| format!("Terminal session {session_id:?} was not found"))?;
        if session.pipe_was_lent {
            bail!(
                "Terminal session {session_id:?} cannot be attached on Windows: its pipe has                  already been lent to one client, and Windows binds a pipe to the first reader                  for the life of the pipe. The session is still running and has been left                  detached."
            );
        }
        let endpoint = Endpoint {
            pid: session.pty.pid(),
            raw: lend_handle_to_process(session.pty.pipe_handle(), client_pid)?,
        };
        session.pipe_was_lent = true;
        session.attached_client = Some(AttachedClient::open(client_pid)?);
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
