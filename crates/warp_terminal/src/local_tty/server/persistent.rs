use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use instant::Instant;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use warp_core::SessionId;

use super::{api, protocol};
use crate::local_tty::{self, PtySpawnResult};

// macOS Unix socket paths must fit in 104 bytes, including the application support directory.
const SOCKET_FILE_NAME: &str = if cfg!(target_os = "macos") {
    "v1"
} else {
    "terminal-session-supervisor-v1.sock"
};
const SOCKET_DIRECTORY_NAME: &str = if cfg!(target_os = "macos") {
    "pty"
} else {
    "terminal-session-supervisor"
};
const OUTPUT_DIRECTORY_NAME: &str = "terminal_session_output";
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a single request/response exchange may take before the peer is
/// abandoned.
///
/// The supervisor serves one connection at a time, so without this a peer that
/// connects and then never speaks -- stalled, or hostile -- parks the daemon
/// forever: `maintain_sessions` stops running, detached PTY buffers fill, and
/// every detached shell blocks on write. The client sets the same bound because
/// `spawn_pty` runs on the app's main thread, where a wedged supervisor would
/// otherwise hang the UI.
pub(super) const IO_TIMEOUT: Duration = Duration::from_secs(15);

/// How much output one detached session may spool before the oldest is dropped.
const MAX_SPOOLED_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;
/// How much of the spool survives compaction. A replay wants the most recent
/// output, so compaction keeps the tail.
const RETAINED_SPOOL_BYTES: u64 = 4 * 1024 * 1024;
const SPOOL_TRUNCATION_NOTICE: &[u8] = b"

[warp] Earlier output from this detached session was discarded.

";

/// How long the supervisor lingers with nothing to supervise before exiting.
///
/// Without this the daemon is immortal: it is `setsid()`'d and nothing reaps it,
/// so an app crash leaves it running past logout. Sessions that are merely
/// detached still count as work, so this only fires once every supervised shell
/// has genuinely exited.
const IDLE_SHUTDOWN_DELAY: Duration = Duration::from_secs(60);

/// The directory holding the supervisor socket.
///
/// Uses `XDG_RUNTIME_DIR` when available, otherwise the app's state directory. The socket must
/// live inside a verified, user-owned 0700 directory: a predictable socket directly in a shared
/// temporary directory could be pre-bound by another user and receive shell credentials.
fn socket_directory() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(warp_core::paths::state_dir);
    base.join(SOCKET_DIRECTORY_NAME)
}

fn socket_path_in(state_directory: &Path, directory: &Path) -> PathBuf {
    let state_directory_hash = Sha256::digest(state_directory.as_os_str().as_bytes());
    directory.join(format!(
        "{}-{}-{}",
        SOCKET_FILE_NAME,
        nix::unistd::getuid().as_raw(),
        hex::encode(&state_directory_hash[..8])
    ))
}

#[cfg(target_os = "macos")]
fn socket_path_with_fallback(
    state_directory: &Path,
    directory: &Path,
    temporary_directory: &Path,
) -> PathBuf {
    let path = socket_path_in(state_directory, directory);
    if nix::sys::socket::UnixAddr::new(&path).is_ok() {
        path
    } else {
        socket_path_in(state_directory, &temporary_directory.join("warp-pty"))
    }
}

pub(super) fn socket_path() -> Result<PathBuf> {
    let state_directory = warp_core::paths::state_dir();
    #[cfg(target_os = "macos")]
    let path =
        socket_path_with_fallback(&state_directory, &socket_directory(), &std::env::temp_dir());
    #[cfg(not(target_os = "macos"))]
    let path = socket_path_in(&state_directory, &socket_directory());

    nix::sys::socket::UnixAddr::new(&path).with_context(|| {
        format!("Terminal supervisor socket path {path:?} exceeds the platform limit; use a shorter runtime or temporary directory")
    })?;
    // Validate before the client connects, including when an existing supervisor is reused.
    prepare_socket_directory(path.parent().expect("socket path has a directory"))?;
    Ok(path)
}

/// A client process, identified by more than its PID.
///
/// PIDs are recycled. A session whose client died was detected as detached by
/// checking whether the PID was still alive, so once the kernel handed that
/// number to an unrelated process the session looked permanently attached:
/// `attach_session` refused every reattach and the shell could never be
/// reached again. Pairing the PID with the process's start time makes a
/// recycled PID distinguishable from the process that actually attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClientProcess {
    pid: u32,
    /// The kernel's start time for `pid`, read when the client attached.
    ///
    /// `None` on platforms where we cannot read it, where this degrades to the
    /// previous PID-only behaviour rather than refusing to work.
    start_time: Option<u64>,
}

impl ClientProcess {
    fn new(pid: u32) -> Self {
        Self {
            pid,
            start_time: process_start_time(pid),
        }
    }

    /// Whether this exact process is still running -- not merely something
    /// holding its PID.
    fn is_running(&self) -> bool {
        if !process_is_running(self.pid) {
            return false;
        }
        match (self.start_time, process_start_time(self.pid)) {
            (Some(attached), Some(current)) => attached == current,
            // Unknown on one side or the other: fall back to liveness alone.
            _ => true,
        }
    }
}

struct SupervisedSession {
    child: Child,
    leader: File,
    attached_client: Option<ClientProcess>,
    preserve_for_client_pid: Option<u32>,
    detached_output: NamedTempFile,
    spooled_output_bytes: u64,
}

impl SupervisedSession {
    fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn update_attachment(&mut self) {
        if self
            .attached_client
            .is_some_and(|client| !client.is_running())
        {
            self.attached_client = None;
        }
    }

    /// Appends to the detached spool, compacting it if it has grown past
    /// [`MAX_SPOOLED_OUTPUT_BYTES`].
    ///
    /// Without a bound a detached `yes` fills the disk and then replays in full
    /// on reattach.
    fn spool_output(&mut self, bytes: &[u8]) -> Result<()> {
        self.detached_output.write_all(bytes)?;
        self.spooled_output_bytes += bytes.len() as u64;
        if self.spooled_output_bytes > MAX_SPOOLED_OUTPUT_BYTES {
            self.compact_spool()?;
        }
        Ok(())
    }

    /// Drops the oldest spooled output, keeping the most recent
    /// [`RETAINED_SPOOL_BYTES`] behind a notice that output was lost.
    fn compact_spool(&mut self) -> Result<()> {
        let file = self.detached_output.as_file_mut();
        file.flush()?;
        let length = file.metadata()?.len();
        if length <= RETAINED_SPOOL_BYTES {
            self.spooled_output_bytes = length;
            return Ok(());
        }

        file.seek(std::io::SeekFrom::Start(length - RETAINED_SPOOL_BYTES))?;
        let mut tail = Vec::with_capacity(RETAINED_SPOOL_BYTES as usize);
        file.read_to_end(&mut tail)?;

        file.set_len(0)?;
        file.seek(std::io::SeekFrom::Start(0))?;
        file.write_all(SPOOL_TRUNCATION_NOTICE)?;
        file.write_all(&tail)?;
        file.flush()?;
        self.spooled_output_bytes = file.metadata()?.len();
        Ok(())
    }

    fn drain_detached_output(&mut self) -> Result<()> {
        if self.attached_client.is_some() {
            return Ok(());
        }

        let mut buffer = [0; 64 * 1024];
        loop {
            match self.leader.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(bytes_read) => self
                    .spool_output(&buffer[..bytes_read])
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
                            attached_client: Some(ClientProcess::new(client_pid)),
                            preserve_for_client_pid: None,
                            detached_output,
                            spooled_output_bytes: 0,
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
                                session.attached_client = None;
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
                            .attached_client
                            .is_none_or(|attached| attached.pid == client_pid)
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
                session.attached_client = None;
                // Consume the mark. It exists so a client can shut itself down
                // without taking its sessions with it, and that is a one-shot
                // request: leaving it set means every later kill for this
                // session reports success without killing anything, and the
                // shell can never be terminated again.
                session.preserve_for_client_pid = None;
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
            if session.attached_client.is_some() {
                bail!("Terminal session is already attached");
            }
            session.drain_detached_output()?;

            session.detached_output.flush()?;
            let replay_size = session.detached_output.as_file().metadata()?.len();
            let mut replay = std::mem::replace(
                &mut session.detached_output,
                NamedTempFile::new_in(&self.output_directory)?,
            );
            session.spooled_output_bytes = 0;
            replay.as_file_mut().seek(std::io::SeekFrom::Start(0))?;

            session.attached_client = Some(ClientProcess::new(client_pid));
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
                    session.attached_client = None;
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

/// Creates the socket's directory as 0700 and refuses to use one that anybody
/// else could write into.
///
/// This is what actually protects the socket: inside a directory only this user
/// may write, no other local user can pre-create the socket path at all.
fn prepare_socket_directory(directory: &Path) -> Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .with_context(|| format!("Failed to create supervisor socket directory {directory:?}"))?;

    // `symlink_metadata`, not `metadata`: a symlink planted here must be
    // rejected rather than followed to whatever it points at.
    let metadata = std::fs::symlink_metadata(directory)
        .with_context(|| format!("Failed to inspect supervisor socket directory {directory:?}"))?;
    if !metadata.file_type().is_dir() {
        bail!("Supervisor socket directory {directory:?} is not a directory");
    }
    if metadata.uid() != nix::unistd::getuid().as_raw() {
        bail!("Supervisor socket directory {directory:?} is owned by another user");
    }
    // `recursive(true)` applies the mode only to directories it creates, so a
    // pre-existing directory can still be group- or world-writable.
    if metadata.mode() & 0o077 != 0 {
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).with_context(
            || format!("Failed to restrict supervisor socket directory {directory:?}"),
        )?;
    }
    Ok(())
}

/// Binds the listener, refusing to displace a live supervisor and refusing to
/// adopt a path we do not own.
fn bind_listener(socket_path: &Path) -> Result<UnixListener> {
    match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() {
                bail!("Supervisor socket path {socket_path:?} is not a socket");
            }
            if metadata.uid() != nix::unistd::getuid().as_raw() {
                bail!("Supervisor socket {socket_path:?} is owned by another user");
            }
            // A successful connect means a supervisor is already serving this
            // path. Previously the unlink was skipped in exactly that case and
            // the bind below was left to fail, which handed the name to whoever
            // got there first; now it is an explicit refusal.
            if UnixStream::connect(socket_path).is_ok() {
                bail!("Another terminal supervisor is already listening on {socket_path:?}");
            }
            std::fs::remove_file(socket_path).with_context(|| {
                format!("Failed to remove stale supervisor socket {socket_path:?}")
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to inspect supervisor socket {socket_path:?}"));
        }
    }

    // Bind under a umask that denies everyone else, so the socket is 0600 from
    // the instant it exists. `chmod`-ing after the bind leaves a window in
    // which the socket is connectable by other users.
    let previous_umask = nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o177));
    let listener = UnixListener::bind(socket_path);
    nix::sys::stat::umask(previous_umask);
    listener.with_context(|| format!("Failed to bind terminal supervisor socket {socket_path:?}"))
}

/// Makes an accepted connection blocking and bounds every read and write on it.
///
/// The listener is non-blocking and on some platforms the accepted socket
/// inherits that, while the exchange below is written against a blocking
/// socket. The timeouts are what stop one silent peer from parking the daemon.
fn configure_stream(stream: &UnixStream) -> Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    Ok(())
}

pub(super) fn run(socket_path: PathBuf) -> Result<()> {
    let socket_directory = socket_path
        .parent()
        .ok_or_else(|| anyhow!("Terminal supervisor socket has no parent directory"))?;
    prepare_socket_directory(socket_directory)?;
    let output_directory = warp_core::paths::state_dir().join(OUTPUT_DIRECTORY_NAME);
    std::fs::create_dir_all(&output_directory)?;

    let listener = bind_listener(&socket_path)?;
    listener.set_nonblocking(true)?;
    let _socket_guard = SocketGuard(&socket_path);
    let mut supervisor = Supervisor::new(output_directory);
    let mut idle_since: Option<Instant> = Some(Instant::now());

    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(error) = configure_stream(&stream) {
                    log::error!("Terminal supervisor connection setup failed: {error:#}");
                    continue;
                }
                if let Err(error) = supervisor.handle_connection(stream) {
                    log::error!("Terminal supervisor request failed: {error:#}");
                }
                idle_since = None;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                supervisor.maintain_sessions();
                if supervisor.sessions.is_empty() {
                    let idle_since = idle_since.get_or_insert_with(Instant::now);
                    if idle_since.elapsed() >= IDLE_SHUTDOWN_DELAY {
                        log::info!("Terminal supervisor exiting: nothing left to supervise");
                        return Ok(());
                    }
                } else {
                    idle_since = None;
                }
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

/// The kernel's start time for a process, used to tell a recycled PID from the
/// process that actually attached.
///
/// Linux only for now: field 22 of `/proc/<pid>/stat`, in clock ticks since
/// boot. Elsewhere this returns `None` and callers fall back to a bare liveness
/// check, which is what every platform did before.
#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The second field is the executable name in parentheses and may itself
    // contain spaces and parentheses, so fields are counted from the last ')'.
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn process_start_time(_pid: u32) -> Option<u64> {
    None
}

fn process_is_running(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as i32, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
#[path = "persistent_tests.rs"]
mod tests;
