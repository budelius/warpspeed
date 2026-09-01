//! The supervisor's control channel on Windows.
//!
//! The Unix side uses a Unix domain socket under the user's own uid at mode
//! `0600`. Windows has no such socket, so the equivalent is a named pipe with
//! an explicit DACL -- see [`super::pipe_security`] for why the system default
//! is not good enough for a channel that grants shell access.
//!
//! The pipe carries only control messages. The PTY's own bytes never travel
//! through here: the supervisor duplicates the pipe handle into the client and
//! the client reads it directly, which is what keeps the supervisor out of the
//! I/O hot path.

use anyhow::{Context, Result, bail};
use instant::Instant;
use sha2::{Digest, Sha256};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_READMODE_BYTE,
    PIPE_TYPE_BYTE, PIPE_WAIT, PeekNamedPipe,
};
use windows::core::PCWSTR;

use super::pipe_security::PipeSecurity;
use crate::local_tty::supervisor::framing::{read_message, write_message};
use crate::local_tty::supervisor::message::{Message, PROTOCOL_VERSION};
use crate::local_tty::supervisor::transport::Transport;

/// How many concurrent clients the supervisor accepts.
const MAX_PIPE_INSTANCES: u32 = 16;

/// Buffer sizes advised to the kernel. Control messages are small; a snapshot
/// is the only large one, and framing handles a partial read.
const PIPE_BUFFER_BYTES: u32 = 64 * 1024;

/// The pipe every client of this supervisor connects to.
///
/// The name is derived from the state directory, so two Warp installations with
/// separate state do not share a supervisor, and carries the protocol version
/// so a v2 client never reaches a v1 supervisor.
pub fn supervisor_pipe_name() -> String {
    let state_directory = warp_core::paths::state_dir();
    let hash = Sha256::digest(state_directory.to_string_lossy().as_bytes());
    format!(
        r"\\.\pipe\warp-terminal-supervisor-v{}-{}",
        PROTOCOL_VERSION,
        hex::encode(&hash[..8])
    )
}

/// Encodes `name` the way the Win32 entry points want it.
fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Creates one unconnected instance of the named pipe.
fn create_instance(name: &str, security: &PipeSecurity) -> Result<HANDLE> {
    let attributes = security.attributes();
    let wide_name = wide(name);

    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(wide_name.as_ptr()),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            MAX_PIPE_INSTANCES,
            PIPE_BUFFER_BYTES,
            PIPE_BUFFER_BYTES,
            0,
            Some(&attributes),
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        bail!(
            "Failed to create the supervisor pipe {name}: {}",
            std::io::Error::last_os_error()
        );
    }

    Ok(handle)
}

/// The supervisor's listening end.
pub struct PipeListener {
    name: String,
    security: PipeSecurity,

    /// An instance created and waiting for its client.
    pending_instance: HANDLE,
}

impl PipeListener {
    /// Starts listening on `name`.
    ///
    /// The instance is created here rather than inside [`Self::accept`], so the
    /// pipe exists in the namespace as soon as this returns. Creating it lazily
    /// leaves a window where the supervisor has advertised a name that a client
    /// cannot yet connect to -- the client fails, and the supervisor then blocks
    /// in `ConnectNamedPipe` for a client that has already given up.
    pub fn bind(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        let security = PipeSecurity::current_user_only()?;
        let pending_instance = create_instance(&name, &security)?;

        Ok(Self {
            name,
            security,
            pending_instance,
        })
    }

    /// Waits for one client and returns the transport for it.
    ///
    /// Blocks until a client connects, so callers run it on their own thread.
    pub fn accept(&mut self) -> Result<PipeTransport> {
        let handle = self.pending_instance;

        // A client that connected before this call reports
        // ERROR_PIPE_CONNECTED, which is success rather than failure.
        if let Err(error) = unsafe { ConnectNamedPipe(handle, None) }
            && error.code() != ERROR_PIPE_CONNECTED.to_hresult()
        {
            return Err(error).context("Failed to accept a client on the supervisor pipe");
        }

        // Hand this instance to the caller and stand up the next one, so the
        // name never disappears from the namespace between clients.
        self.pending_instance = create_instance(&self.name, &self.security)?;

        Ok(PipeTransport {
            handle,
            pending: Vec::new(),
        })
    }
}

impl Drop for PipeListener {
    fn drop(&mut self) {
        if !self.pending_instance.is_invalid() {
            let _ = unsafe { CloseHandle(self.pending_instance) };
        }
    }
}

/// One end of a connected supervisor pipe.
pub struct PipeTransport {
    handle: HANDLE,

    /// Bytes received but not yet forming a whole message.
    pending: Vec<u8>,
}

impl PipeTransport {
    /// Connects to a supervisor listening on `name`.
    pub fn connect(name: &str) -> Result<Self> {
        let wide_name = wide(name);

        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide_name.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        }
        .with_context(|| format!("Failed to connect to the supervisor pipe {name}"))?;

        Ok(Self {
            handle,
            pending: Vec::new(),
        })
    }

    /// Returns the process ID connected to the client side of an accepted pipe.
    pub fn client_process_id(&self) -> Result<u32> {
        let mut process_id = 0u32;
        unsafe { GetNamedPipeClientProcessId(self.handle, &mut process_id) }
            .context("Failed to identify the terminal supervisor client")?;
        Ok(process_id)
    }

    /// Waits for one complete message until `timeout` expires.
    pub fn receive_timeout(&mut self, timeout: std::time::Duration) -> Result<Message> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(message) = self.receive()? {
                return Ok(message);
            }
            if Instant::now() >= deadline {
                bail!("Timed out waiting for a terminal supervisor message");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Moves whatever the pipe already holds into `pending`.
    ///
    /// Uses `PeekNamedPipe` to learn how much is available rather than putting
    /// the pipe in `PIPE_NOWAIT`, which Windows documents as present only for
    /// compatibility and which loses message boundaries.
    fn drain_available(&mut self) -> Result<()> {
        loop {
            let mut available = 0u32;
            unsafe { PeekNamedPipe(self.handle, None, 0, None, Some(&mut available), None) }
                .context("Failed to peek at the supervisor pipe")?;

            if available == 0 {
                return Ok(());
            }

            let mut chunk = vec![0u8; available as usize];
            let mut read = 0u32;
            unsafe { ReadFile(self.handle, Some(&mut chunk), Some(&mut read), None) }
                .context("Failed to read from the supervisor pipe")?;

            if read == 0 {
                return Ok(());
            }
            chunk.truncate(read as usize);
            self.pending.extend_from_slice(&chunk);
        }
    }
}

impl Transport for PipeTransport {
    fn send(&mut self, message: &Message) -> Result<()> {
        let mut framed = Vec::new();
        write_message(&mut framed, message)?;

        let mut written_total = 0usize;
        while written_total < framed.len() {
            let mut written = 0u32;
            unsafe {
                WriteFile(
                    self.handle,
                    Some(&framed[written_total..]),
                    Some(&mut written),
                    None,
                )
            }
            .context("Failed to write to the supervisor pipe")?;

            if written == 0 {
                bail!("The supervisor pipe accepted no bytes; the peer has gone");
            }
            written_total += written as usize;
        }

        Ok(())
    }

    fn receive(&mut self) -> Result<Option<Message>> {
        self.drain_available()?;

        let mut cursor = std::io::Cursor::new(&self.pending[..]);
        let message = read_message(&mut cursor)?;

        // Consume only what decoded, so a message still arriving stays buffered
        // rather than having its front thrown away.
        if message.is_some() {
            let consumed = cursor.position() as usize;
            self.pending.drain(..consumed);
        }

        Ok(message)
    }

    fn lends_endpoints(&self) -> bool {
        // The supervisor duplicates the PTY handle into the client with
        // `DuplicateHandle`, and the value rides back in the attach response.
        true
    }
}

impl Drop for PipeTransport {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            let _ = unsafe { CloseHandle(self.handle) };
        }
    }
}

#[cfg(test)]
#[path = "pipe_transport_tests.rs"]
mod tests;
