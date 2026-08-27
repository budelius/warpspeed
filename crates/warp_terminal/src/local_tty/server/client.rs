use std::collections::HashSet;
use std::os::unix::net::UnixStream;
use std::os::unix::prelude::*;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, bail};
use parking_lot::Mutex;
use warp_core::SessionId;

use super::{api, protocol};
use crate::local_tty::{PtyOptions, PtySpawnResult};

enum Connection {
    Ephemeral(Mutex<OwnedFd>),
    Persistent(PathBuf),
}

/// A client for communicating with the terminal server.
pub struct TerminalServerClient {
    connection: Connection,
    /// The set of process IDs of terminated children which have not yet been
    /// processed by the pty event loops.
    terminated_children: Arc<Mutex<HashSet<u32>>>,
}

impl TerminalServerClient {
    /// Constructs a client for the terminal server tied to this app process.
    pub fn new(client_fd: OwnedFd, terminated_children: Arc<Mutex<HashSet<u32>>>) -> Self {
        Self {
            connection: Connection::Ephemeral(Mutex::new(client_fd)),
            terminated_children,
        }
    }

    /// Constructs a client for the per-user terminal session supervisor.
    pub fn new_persistent(socket_path: PathBuf) -> Self {
        Self {
            connection: Connection::Persistent(socket_path),
            terminated_children: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn is_persistent(&self) -> bool {
        matches!(self.connection, Connection::Persistent(_))
    }

    fn request(&self, message: api::Message) -> Result<Option<api::Message>> {
        match &self.connection {
            Connection::Ephemeral(fd) => {
                let fd = fd.lock();
                protocol::send_message(fd.as_fd(), message, Option::<RawFd>::None)?;
                protocol::receive_message(fd.as_fd())
            }
            Connection::Persistent(socket_path) => {
                let stream = UnixStream::connect(socket_path)?;
                protocol::send_message(stream.as_fd(), message, Option::<RawFd>::None)?;
                protocol::receive_message(stream.as_fd())
            }
        }
    }

    /// Asks the server to spawn a pty, returning the pty leader file descriptor
    /// and other metadata upon success.
    pub fn spawn_pty(
        &self,
        options: PtyOptions,
        persistent_session_id: Option<SessionId>,
    ) -> Result<PtySpawnResult> {
        let result = self.request(api::Message::SpawnShellRequest {
            options: Box::new(options),
            persistent_session_id,
            client_pid: std::process::id(),
        })?;
        match result {
            Some(api::Message::SpawnShellResponse {
                spawn_result: api::Result::Ok(spawn_result),
            }) => Ok(spawn_result),
            Some(api::Message::SpawnShellResponse {
                spawn_result: api::Result::Err(message),
            }) => {
                bail!("Terminal server failed to spawn a shell: {message}");
            }
            Some(_) => {
                bail!("Got response other than SpawnShellResponse after sending SpawnShellRequest");
            }
            None => bail!("Terminal server closed while spawning a shell"),
        }
    }

    pub fn attach_session(&self, session_id: SessionId) -> Result<Option<PtySpawnResult>> {
        if !self.is_persistent() {
            return Ok(None);
        }
        let result = self.request(api::Message::AttachSessionRequest {
            session_id,
            client_pid: std::process::id(),
        })?;
        match result {
            Some(api::Message::AttachSessionResponse {
                attach_result: api::Result::Ok(spawn_result),
            }) => Ok(Some(spawn_result)),
            Some(api::Message::AttachSessionResponse {
                attach_result: api::Result::Err(message),
            }) => {
                log::info!("Unable to reattach supervised terminal session: {message}");
                Ok(None)
            }
            Some(_) => bail!("Terminal supervisor returned an invalid attach response"),
            None => bail!("Terminal supervisor closed while attaching a session"),
        }
    }

    /// Asks the server to terminate and clean up its child process with the
    /// given process ID.
    pub fn kill_child(&self, pid: u32) -> Result<()> {
        if self.has_child_terminated(pid, None)? {
            return Ok(());
        }

        let result = match self.request(api::Message::KillChildRequest {
            pid,
            client_pid: std::process::id(),
        }) {
            Ok(result) => result,
            Err(error) if error.downcast_ref::<nix::Error>() == Some(&nix::Error::EPIPE) => {
                log::warn!(
                    "Received EPIPE when trying to kill child shell process; assuming the terminal server has terminated."
                );
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        match result {
            Some(api::Message::KillChildResponse { error_msg }) => match error_msg {
                Some(error_msg) => Err(anyhow::anyhow!(error_msg)),
                None => Ok(()),
            },
            Some(_) => bail!("Terminal server returned an invalid kill response"),
            None => bail!("Terminal server closed while killing a child"),
        }
    }

    pub fn kill_session(&self, session_id: SessionId) -> Result<()> {
        if !self.is_persistent() {
            return Ok(());
        }
        match self.request(api::Message::KillSessionRequest {
            session_id,
            client_pid: std::process::id(),
        })? {
            Some(api::Message::KillSessionResponse { error_msg: None }) => Ok(()),
            Some(api::Message::KillSessionResponse {
                error_msg: Some(error),
            }) => Err(anyhow::anyhow!(error)),
            Some(_) => bail!("Terminal supervisor returned an invalid kill-session response"),
            None => bail!("Terminal supervisor closed while killing a session"),
        }
    }

    /// Returns whether the child has terminated.
    pub fn has_child_terminated(&self, pid: u32, session_id: Option<SessionId>) -> Result<bool> {
        match &self.connection {
            Connection::Ephemeral(_) => Ok(self.terminated_children.lock().remove(&pid)),
            Connection::Persistent(_) => {
                let _ = session_id;
                let result = unsafe { libc::kill(pid as i32, 0) };
                let running = result == 0
                    || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
                Ok(!running)
            }
        }
    }

    pub fn prepare_for_app_termination(
        &self,
        persistent_session_ids: Vec<SessionId>,
    ) -> Result<()> {
        if !self.is_persistent() {
            return Ok(());
        }
        match self.request(api::Message::PrepareForTerminationRequest {
            client_pid: std::process::id(),
            persistent_session_ids,
        })? {
            Some(api::Message::PrepareForTerminationResponse) => Ok(()),
            Some(_) => bail!("Terminal supervisor returned an invalid termination response"),
            None => bail!("Terminal supervisor closed while preparing for termination"),
        }
    }

    pub fn ping(&self) -> Result<()> {
        match self.request(api::Message::PingRequest)? {
            Some(api::Message::PingResponse) => Ok(()),
            Some(_) => bail!("Terminal supervisor returned an invalid ping response"),
            None => bail!("Terminal supervisor closed while responding to ping"),
        }
    }
}
