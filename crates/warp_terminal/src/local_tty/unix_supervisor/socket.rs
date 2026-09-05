//! Where the supervisor listens, and who is allowed to reach it.

use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::prelude::*;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::local_tty::supervisor::message::PROTOCOL_VERSION;

const SOCKET_DIRECTORY_NAME: &str = "warp-supervisor";

/// `sockaddr_un::sun_path` is 108 bytes on Linux and 104 on macOS, including
/// the terminating NUL. A path over that does not fail at bind with anything
/// legible -- it fails with `SUN_LEN`, which says nothing about which of the
/// three parts got too long -- so the check below names the problem instead.
const MAX_SOCKET_PATH_BYTES: usize = 103;

/// The directory holding the supervisor socket.
///
/// `XDG_RUNTIME_DIR` when the platform provides one -- it is per-user and
/// already 0700 -- and the app's state directory otherwise. Deliberately never
/// `/tmp`: both components of the socket name are derivable by any local user,
/// so in a world-writable directory an attacker could pre-bind the path, be
/// handed a spawn request carrying the complete environment (injected API keys
/// and cloud credentials included), and answer with a descriptor of their
/// choosing as the terminal the user types into.
fn socket_directory() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(warp_core::paths::state_dir);
    base.join(SOCKET_DIRECTORY_NAME)
}

/// The socket a client connects to.
///
/// The protocol version is in the name, so a client never speaks v2 to a v1
/// supervisor: after an upgrade the old daemon keeps its own name and its own
/// sessions until its TTL reaps them.
pub fn socket_path() -> PathBuf {
    let state_directory_hash = Sha256::digest(warp_core::paths::state_dir().as_os_str().as_bytes());
    socket_directory().join(format!(
        "v{}-{}-{}.sock",
        PROTOCOL_VERSION,
        nix::unistd::getuid().as_raw(),
        hex::encode(&state_directory_hash[..6])
    ))
}

/// Rejects a path the kernel cannot hold in a `sockaddr_un`.
fn check_socket_path_length(socket_path: &Path) -> Result<()> {
    let length = socket_path.as_os_str().as_bytes().len();
    if length > MAX_SOCKET_PATH_BYTES {
        bail!(
            "Supervisor socket path {socket_path:?} is {length} bytes, over the \
             {MAX_SOCKET_PATH_BYTES}-byte limit a Unix socket address can hold. Set \
             XDG_RUNTIME_DIR to a shorter directory."
        );
    }
    Ok(())
}

/// Creates the socket directory, refusing one this user does not own.
pub fn prepare_socket_directory(directory: &Path) -> Result<()> {
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
/// adopt a path this user does not own.
pub fn bind_listener(socket_path: &Path) -> Result<UnixListener> {
    check_socket_path_length(socket_path)?;
    match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() {
                bail!("Supervisor socket path {socket_path:?} is not a socket");
            }
            if metadata.uid() != nix::unistd::getuid().as_raw() {
                bail!("Supervisor socket {socket_path:?} is owned by another user");
            }
            // A successful connect means a supervisor is already serving this
            // path; taking the name from it would strand its sessions.
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

/// The process id on the other end of a connection.
///
/// Read from the kernel rather than taken from the client's own message: the
/// supervisor decides whether a session is orphaned by watching this process,
/// so a client that could name any pid could keep someone else's session
/// looking attached, or steal one by claiming a pid that is about to die.
#[cfg(target_os = "linux")]
pub fn peer_pid(stream: &UnixStream) -> Result<u32> {
    let credentials = nix::sys::socket::getsockopt(
        stream.as_raw_fd(),
        nix::sys::socket::sockopt::PeerCredentials,
    )
    .context("Failed to read the credentials of a terminal supervisor client")?;
    Ok(credentials.pid() as u32)
}

#[cfg(target_vendor = "apple")]
pub fn peer_pid(stream: &UnixStream) -> Result<u32> {
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // `SOL_LOCAL` / `LOCAL_PEERPID` is the Darwin equivalent of `SO_PEERCRED`.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&raw mut pid).cast(),
            &raw mut length,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error())
            .context("Failed to read the pid of a terminal supervisor client");
    }
    Ok(pid as u32)
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
pub fn peer_pid(_stream: &UnixStream) -> Result<u32> {
    anyhow::bail!("Reading a socket peer's pid is not supported on this platform")
}

/// Removes the socket when the supervisor stops serving it.
pub struct SocketGuard(pub PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
#[path = "socket_tests.rs"]
mod tests;
