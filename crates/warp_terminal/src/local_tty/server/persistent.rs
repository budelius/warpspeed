use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use warp_core::SessionId;

use super::{api, protocol};
use crate::local_tty::{self, PtySpawnResult};

const SOCKET_FILE_NAME: &str = "terminal-session-supervisor-v1.sock";
const OUTPUT_DIRECTORY_NAME: &str = "terminal_session_output";
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(super) fn socket_path() -> PathBuf {
    let state_directory_hash = Sha256::digest(warp_core::paths::state_dir().as_os_str().as_bytes());
    PathBuf::from("/tmp").join(format!(
        "{}-{}-{}",
        SOCKET_FILE_NAME,
        nix::unistd::getuid().as_raw(),
        hex::encode(&state_directory_hash[..8])
    ))
}

struct SupervisedSession {
    child: Child,
    leader: File,
    attached_client_pid: Option<u32>,
    preserve_for_client_pid: Option<u32>,
    detached_output: NamedTempFile,
}

impl SupervisedSession {
    fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn update_attachment(&mut self) {
        if self
            .attached_client_pid
            .is_some_and(|pid| !process_is_running(pid))
        {
            self.attached_client_pid = None;
        }
    }

    fn drain_detached_output(&mut self) -> Result<()> {
        if self.attached_client_pid.is_some() {
            return Ok(());
        }

        let mut buffer = [0; 64 * 1024];
        loop {
            match self.leader.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(bytes_read) => self
                    .detached_output
                    .write_all(&buffer[..bytes_read])
                    .context("Failed to spool detached terminal output")?,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("Failed to read detached terminal output"),
            }
        }
    }
}

struct Supervisor {
    sessions: HashMap<SessionId, SupervisedSession>,
    output_directory: PathBuf,
}

impl Supervisor {
    fn new(output_directory: PathBuf) -> Self {
        Self {
            sessions: HashMap::new(),
            output_directory,
        }
    }

    fn maintain_sessions(&mut self) {
        self.sessions.retain(|session_id, session| {
            if !session.is_running() {
                log::info!("Supervised terminal session {session_id:?} exited");
                return false;
            }
            session.update_attachment();
            if let Err(error) = session.drain_detached_output() {
                log::error!(
                    "Failed to preserve output for terminal session {session_id:?}: {error:#}"
                );
            }
            true
        });
    }

    fn handle_connection(&mut self, stream: UnixStream) -> Result<()> {
        let Some(message) = protocol::receive_message(stream.as_fd())? else {
            return Ok(());
        };
        match message {
            api::Message::SpawnShellRequest {
                mut options,
                persistent_session_id,
                client_pid,
            } => {
                let result = (|| {
                    let session_id = persistent_session_id
                        .ok_or_else(|| anyhow!("Persistent terminal session ID is required"))?;
                    if self.sessions.contains_key(&session_id) {
                        bail!("Terminal session ID is already in use");
                    }

                    options.close_fds = true;
                    let spawned = local_tty::spawn(*options)?;
                    let mut result = spawned.result;
                    set_nonblocking(result.leader_fd)?;
                    let leader = unsafe { File::from_raw_fd(result.leader_fd) };
                    let detached_output = NamedTempFile::new_in(&self.output_directory)?;
                    let response_fd = leader.as_raw_fd();
                    self.sessions.insert(
                        session_id,
                        SupervisedSession {
                            child: spawned.child,
                            leader,
                            attached_client_pid: Some(client_pid),
                            preserve_for_client_pid: None,
                            detached_output,
                        },
                    );
                    result.leader_fd = response_fd;
                    result.replay_fd = None;
                    Ok((session_id, result))
                })();

                match result {
                    Ok((session_id, result)) => {
                        if let Err(error) = protocol::send_message(
                            stream.as_fd(),
                            api::Message::SpawnShellResponse {
                                spawn_result: api::Result::Ok(result.clone()),
                            },
                            Some(result.leader_fd),
                        ) {
                            if let Some(session) = self.sessions.get_mut(&session_id) {
                                session.attached_client_pid = None;
                            }
                            return Err(error);
                        }
                    }
                    Err(error) => protocol::send_message(
                        stream.as_fd(),
                        api::Message::SpawnShellResponse {
                            spawn_result: api::Result::Err(error.to_string()),
                        },
                        None,
                    )?,
                }
            }
            api::Message::AttachSessionRequest {
                session_id,
                client_pid,
            } => self.attach_session(stream, session_id, client_pid)?,
            api::Message::ChildStatusRequest { session_id } => {
                let running = self
                    .sessions
                    .get_mut(&session_id)
                    .is_some_and(SupervisedSession::is_running);
                protocol::send_message(
                    stream.as_fd(),
                    api::Message::ChildStatusResponse { running },
                    None,
                )?;
            }
            api::Message::KillChildRequest { pid, client_pid } => {
                let error_msg = self
                    .sessions
                    .iter()
                    .find_map(|(session_id, session)| {
                        (session.child.id() == pid).then_some(*session_id)
                    })
                    .and_then(|session_id| self.kill_session(session_id, client_pid));
                protocol::send_message(
                    stream.as_fd(),
                    api::Message::KillChildResponse { error_msg },
                    None,
                )?;
            }
            api::Message::KillSessionRequest {
                session_id,
                client_pid,
            } => {
                let error_msg = self.kill_session(session_id, client_pid);
                protocol::send_message(
                    stream.as_fd(),
                    api::Message::KillSessionResponse { error_msg },
                    None,
                )?;
            }
            api::Message::PrepareForTerminationRequest {
                client_pid,
                persistent_session_ids,
            } => {
                for session_id in persistent_session_ids {
                    if let Some(session) = self.sessions.get_mut(&session_id)
                        && session
                            .attached_client_pid
                            .is_none_or(|attached_pid| attached_pid == client_pid)
                    {
                        session.preserve_for_client_pid = Some(client_pid);
                    }
                }
                protocol::send_message(
                    stream.as_fd(),
                    api::Message::PrepareForTerminationResponse,
                    None,
                )?;
            }
            api::Message::PingRequest => {
                protocol::send_message(stream.as_fd(), api::Message::PingResponse, None)?;
            }
            api::Message::SpawnShellResponse { .. }
            | api::Message::AttachSessionResponse { .. }
            | api::Message::ChildStatusResponse { .. }
            | api::Message::PrepareForTerminationResponse
            | api::Message::PingResponse
            | api::Message::KillChildResponse { .. }
            | api::Message::KillSessionResponse { .. }
            | api::Message::WriteLogRequest { .. }
            | api::Message::ChildrenTerminatedRequest { .. } => {
                bail!("Terminal supervisor received a response-only message");
            }
        }
        Ok(())
    }

    fn kill_session(&mut self, session_id: SessionId, client_pid: u32) -> Option<String> {
        let preserving =
            self.sessions.get(&session_id)?.preserve_for_client_pid == Some(client_pid);
        if preserving {
            if let Some(session) = self.sessions.get_mut(&session_id) {
                session.attached_client_pid = None;
            }
            return None;
        }
        let mut session = self.sessions.remove(&session_id)?;
        session
            .child
            .kill()
            .and_then(|_| session.child.wait())
            .err()
            .map(|error| error.to_string())
    }

    fn attach_session(
        &mut self,
        stream: UnixStream,
        session_id: SessionId,
        client_pid: u32,
    ) -> Result<()> {
        let result = (|| {
            let session = self
                .sessions
                .get_mut(&session_id)
                .ok_or_else(|| anyhow!("Terminal session was not found"))?;
            if !session.is_running() {
                bail!("Terminal session is no longer running");
            }
            session.update_attachment();
            if session.attached_client_pid.is_some() {
                bail!("Terminal session is already attached");
            }
            session.drain_detached_output()?;

            session.detached_output.flush()?;
            let replay_size = session.detached_output.as_file().metadata()?.len();
            let mut replay = std::mem::replace(
                &mut session.detached_output,
                NamedTempFile::new_in(&self.output_directory)?,
            );
            replay.as_file_mut().seek(std::io::SeekFrom::Start(0))?;

            session.attached_client_pid = Some(client_pid);
            session.preserve_for_client_pid = None;
            let result = PtySpawnResult {
                pid: session.child.id(),
                leader_fd: session.leader.as_raw_fd(),
                replay_fd: (replay_size > 0).then(|| replay.as_file().as_raw_fd()),
            };
            Ok((result, replay))
        })();

        match result {
            Ok((result, replay)) => {
                let mut fds = vec![result.leader_fd];
                if let Some(replay_fd) = result.replay_fd {
                    fds.push(replay_fd);
                }
                let send_result = protocol::send_message_with_fds(
                    stream.as_fd(),
                    api::Message::AttachSessionResponse {
                        attach_result: api::Result::Ok(result),
                    },
                    &fds,
                );
                drop(replay);
                if send_result.is_err()
                    && let Some(session) = self.sessions.get_mut(&session_id)
                {
                    session.attached_client_pid = None;
                }
                send_result?;
            }
            Err(error) => protocol::send_message(
                stream.as_fd(),
                api::Message::AttachSessionResponse {
                    attach_result: api::Result::Err(error.to_string()),
                },
                None,
            )?,
        }
        Ok(())
    }
}

pub(super) fn run(socket_path: PathBuf) -> Result<()> {
    let socket_directory = socket_path
        .parent()
        .ok_or_else(|| anyhow!("Terminal supervisor socket has no parent directory"))?;
    std::fs::create_dir_all(socket_directory)?;
    let output_directory = warp_core::paths::state_dir().join(OUTPUT_DIRECTORY_NAME);
    std::fs::create_dir_all(&output_directory)?;

    if socket_path.exists() && UnixStream::connect(&socket_path).is_err() {
        std::fs::remove_file(&socket_path)?;
    }
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("Failed to bind terminal supervisor socket {socket_path:?}"))?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let _socket_guard = SocketGuard(&socket_path);
    let mut supervisor = Supervisor::new(output_directory);

    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(error) = supervisor.handle_connection(stream) {
                    log::error!("Terminal supervisor request failed: {error:#}");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                supervisor.maintain_sessions();
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error).context("Terminal supervisor listener failed"),
        }
    }
}

struct SocketGuard<'a>(&'a Path);

impl Drop for SocketGuard<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

fn set_nonblocking(fd: RawFd) -> Result<()> {
    let flags = nix::fcntl::fcntl(fd, nix::fcntl::F_GETFL)?;
    let mut flags = nix::fcntl::OFlag::from_bits_truncate(flags);
    flags.insert(nix::fcntl::OFlag::O_NONBLOCK);
    nix::fcntl::fcntl(fd, nix::fcntl::F_SETFL(flags))?;
    Ok(())
}

fn process_is_running(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as i32, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
#[path = "persistent_tests.rs"]
mod tests;
