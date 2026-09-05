//! The supervisor protocol over an `AF_UNIX` socket.
//!
//! ## How an endpoint travels
//!
//! A descriptor cannot be put in a serialized message: its number means nothing
//! in another process. So an [`Endpoint`] is sent in two halves. The message
//! carries the child's pid and a placeholder, and the descriptor itself rides
//! alongside in an `SCM_RIGHTS` control message. The receiving transport pairs
//! them back up as it decodes, which is why nothing above this file has to know
//! that a descriptor was involved at all.
//!
//! This is the Unix answer to what `DuplicateHandle` does on Windows, and the
//! reason [`Transport::lends_endpoints`] exists rather than being assumed.

use std::io::{IoSlice, IoSliceMut};
use std::os::unix::net::UnixStream;
use std::os::unix::prelude::*;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use instant::Instant;
use nix::cmsg_space;
use nix::errno::Errno;
use nix::sys::socket;

use crate::local_tty::supervisor::message::Message;
use crate::local_tty::supervisor::transport::{Endpoint, Transport};

/// The length prefix each message carries, in bytes.
const LENGTH_PREFIX_SIZE: usize = std::mem::size_of::<u32>();

/// A refusal to allocate for a length that could only be a framing error or a
/// hostile peer.
const MAX_MESSAGE_BYTES: u32 = 64 * 1024 * 1024;

/// How long either side waits on a single read or write before giving up.
const IO_TIMEOUT: Duration = Duration::from_secs(15);

/// What an attempt to read a whole buffer ended in.
enum ReadOutcome {
    /// The buffer was filled.
    Complete,
    /// Nothing more has arrived yet, and the peer is still there.
    WouldBlock,
    /// The peer closed the connection.
    Closed,
}

/// The supervisor protocol over a connected Unix domain socket.
pub struct UnixSocketTransport {
    stream: UnixStream,

    /// Set once a read has seen the peer hang up, so a caller waiting for a
    /// message can stop rather than wait out its whole timeout.
    peer_closed: bool,
}

impl UnixSocketTransport {
    pub fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            peer_closed: false,
        }
    }

    /// Connects to a supervisor listening at `path`.
    ///
    /// Both directions are bounded. Requests run on the app's main thread, so a
    /// supervisor that accepts a connection and then says nothing would
    /// otherwise block the UI for as long as it stayed silent.
    pub fn connect(path: &std::path::Path) -> Result<Self> {
        let stream = UnixStream::connect(path)
            .with_context(|| format!("Failed to reach the terminal supervisor at {path:?}"))?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        Ok(Self::new(stream))
    }

    /// Sends a message, lending `endpoint_fd` alongside it when there is one.
    ///
    /// Only the supervisor ever passes a descriptor; a client's requests never
    /// carry one.
    pub fn send_with_endpoint(
        &mut self,
        message: &Message,
        endpoint_fd: Option<RawFd>,
    ) -> Result<()> {
        let payload =
            bincode::serialize(message).context("Failed to encode a supervisor message")?;
        let length = u32::try_from(payload.len())
            .context("Supervisor message is too large to frame")?
            .to_be_bytes();

        let mut framed = Vec::with_capacity(LENGTH_PREFIX_SIZE + payload.len());
        framed.extend_from_slice(&length);
        framed.extend_from_slice(&payload);

        let fds = endpoint_fd.as_slice();
        let control = if fds.is_empty() {
            Vec::new()
        } else {
            vec![socket::ControlMessage::ScmRights(fds)]
        };

        let mut sent = 0;
        while sent < framed.len() {
            let iov = [IoSlice::new(&framed[sent..])];
            // The control message rides on the first write only: repeating it
            // would lend the descriptor more than once.
            let control = if sent == 0 { control.as_slice() } else { &[] };
            let written = socket::sendmsg::<()>(
                self.stream.as_raw_fd(),
                &iov,
                control,
                socket::MsgFlags::empty(),
                None,
            )
            .context("Failed to write to the terminal supervisor socket")?;
            ensure!(written > 0, "Terminal supervisor socket accepted no bytes");
            sent += written;
        }
        Ok(())
    }

    /// Receives one message, waiting up to `timeout` for it to arrive whole.
    ///
    /// A peer that hangs up is an immediate answer rather than something to
    /// keep waiting on. Without that the supervisor's single-threaded loop
    /// would spin out the whole timeout every time a client died mid-exchange,
    /// and nothing would be feeding the detached terminals meanwhile.
    pub fn receive_timeout(&mut self, timeout: Duration) -> Result<Message> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.receive()? {
                Some(message) => return Ok(message),
                None if self.peer_closed => {
                    bail!("The terminal supervisor connection closed before answering")
                }
                None => {}
            }
            if Instant::now() >= deadline {
                bail!("Timed out waiting for a terminal supervisor response");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Reads exactly `buffer.len()` bytes, collecting any control messages that
    /// arrive with them.
    ///
    /// Control messages are attached to a write, not to a byte offset, so a
    /// descriptor can surface on any read that overlaps the sender's first
    /// write -- which is why they are gathered across the whole loop rather
    /// than only from the first call.
    fn read_exact(&mut self, buffer: &mut [u8], fds: &mut Vec<RawFd>) -> Result<ReadOutcome> {
        let mut filled = 0;
        while filled < buffer.len() {
            let mut cmsg_buffer = cmsg_space!([RawFd; 2]);
            let mut slices = [IoSliceMut::new(&mut buffer[filled..])];
            let received: socket::RecvMsg<()> = match socket::recvmsg(
                self.stream.as_raw_fd(),
                &mut slices,
                Some(&mut cmsg_buffer),
                socket::MsgFlags::empty(),
            ) {
                Ok(received) => received,
                Err(Errno::EINTR) => continue,
                // The socket's own read timeout, or a non-blocking socket
                // with nothing on it. Either way the caller decides whether to
                // keep waiting.
                Err(Errno::EAGAIN) => return Ok(ReadOutcome::WouldBlock),
                Err(error) => {
                    return Err(error)
                        .context("Failed to read from the terminal supervisor socket");
                }
            };

            for control in received.cmsgs() {
                if let socket::ControlMessageOwned::ScmRights(received_fds) = control {
                    fds.extend(received_fds);
                }
            }

            if received.bytes == 0 {
                // The peer closed. Anything already lent would leak.
                for fd in fds.drain(..) {
                    let _ = nix::unistd::close(fd);
                }
                self.peer_closed = true;
                return Ok(ReadOutcome::Closed);
            }
            filled += received.bytes;
        }
        Ok(ReadOutcome::Complete)
    }
}

impl Transport for UnixSocketTransport {
    fn send(&mut self, message: &Message) -> Result<()> {
        self.send_with_endpoint(
            message,
            endpoint_of(message).map(|endpoint| endpoint.raw as RawFd),
        )
    }

    fn receive(&mut self) -> Result<Option<Message>> {
        let mut fds = Vec::new();
        let mut length_bytes = [0u8; LENGTH_PREFIX_SIZE];
        match self.read_exact(&mut length_bytes, &mut fds)? {
            ReadOutcome::Complete => {}
            ReadOutcome::WouldBlock | ReadOutcome::Closed => return Ok(None),
        }
        let length = u32::from_be_bytes(length_bytes);
        ensure!(
            length > 0 && length <= MAX_MESSAGE_BYTES,
            "Terminal supervisor sent an implausible message length of {length} bytes"
        );

        let mut payload = vec![0u8; length as usize];
        if !matches!(
            self.read_exact(&mut payload, &mut fds)?,
            ReadOutcome::Complete
        ) {
            bail!("Terminal supervisor closed part-way through a message");
        }

        let mut message: Message =
            bincode::deserialize(&payload).context("Failed to decode a supervisor message")?;

        // Put the descriptor back where the sender took it from. A message that
        // should carry one and did not is a protocol error, not a session with
        // an unusable endpoint: dropping it silently would leave the client
        // reading descriptor 0.
        if let Some(endpoint) = endpoint_of_mut(&mut message) {
            let Some(fd) = fds.pop() else {
                bail!("Terminal supervisor lent no descriptor alongside an endpoint");
            };
            endpoint.raw = fd as i64;
        }
        for leaked in fds {
            let _ = nix::unistd::close(leaked);
        }

        Ok(Some(message))
    }

    fn lends_endpoints(&self) -> bool {
        true
    }
}

/// The endpoint a message carries, if it carries one.
fn endpoint_of(message: &Message) -> Option<&Endpoint> {
    match message {
        Message::SpawnResponse {
            result: Ok(endpoint),
        } => Some(endpoint),
        Message::AttachResponse {
            result: Ok(session),
        } => session.endpoint.as_ref(),
        _ => None,
    }
}

fn endpoint_of_mut(message: &mut Message) -> Option<&mut Endpoint> {
    match message {
        Message::SpawnResponse {
            result: Ok(endpoint),
        } => Some(endpoint),
        Message::AttachResponse {
            result: Ok(session),
        } => session.endpoint.as_mut(),
        _ => None,
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
