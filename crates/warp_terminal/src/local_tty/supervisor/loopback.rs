//! An in-memory [`Transport`] that lends no endpoint.
//!
//! This exists to keep the remote design honest. Every off-machine transport
//! -- SSH, or a tunnel through a fleet controller -- is unable to pass a file
//! descriptor, so it must work through streamed messages alone. Proving that
//! path here, with no sockets and no processes involved, means a remote
//! transport is a matter of moving bytes rather than a redesign.
//!
//! It runs the real framing over a shared byte queue rather than passing
//! structs across, so encode and decode are genuinely exercised.

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::Arc;

use anyhow::Result;
use parking_lot::Mutex;
use warp_core::SessionId;

use super::framing::{read_message, write_message};
use super::message::Message;
use super::transport::{Endpoint, Transport};

/// A byte queue shared by the two ends of a loopback pair.
type Channel = Arc<Mutex<VecDeque<u8>>>;

/// One end of an in-memory transport pair.
pub struct LoopbackTransport {
    outgoing: Channel,
    incoming: Channel,

    /// Bytes received but not yet forming a whole message.
    ///
    /// Without this, a partial message read straight from the shared queue
    /// would consume its bytes and return `None`, losing the front of a message
    /// that had merely not finished arriving.
    pending: Vec<u8>,
}

impl LoopbackTransport {
    /// Creates two connected ends. What one sends, the other receives.
    pub fn pair() -> (Self, Self) {
        let one_way: Channel = Arc::new(Mutex::new(VecDeque::new()));
        let other_way: Channel = Arc::new(Mutex::new(VecDeque::new()));

        let first = Self {
            outgoing: one_way.clone(),
            incoming: other_way.clone(),
            pending: Vec::new(),
        };
        let second = Self {
            outgoing: other_way,
            incoming: one_way,
            pending: Vec::new(),
        };

        (first, second)
    }
}

impl Transport for LoopbackTransport {
    fn send(&mut self, message: &Message) -> Result<()> {
        let mut framed = Vec::new();
        write_message(&mut framed, message)?;
        self.outgoing.lock().extend(framed);
        Ok(())
    }

    fn receive(&mut self) -> Result<Option<Message>> {
        {
            let mut incoming = self.incoming.lock();
            self.pending.extend(incoming.drain(..));
        }

        let mut cursor = Cursor::new(&self.pending[..]);
        let message = read_message(&mut cursor)?;

        // Consume only what actually decoded, so an incomplete message stays
        // buffered for the next call rather than being thrown away.
        if message.is_some() {
            let consumed = cursor.position() as usize;
            self.pending.drain(..consumed);
        }

        Ok(message)
    }

    fn lend_endpoint(&mut self, _session_id: SessionId) -> Option<Result<Endpoint>> {
        // A loopback has no PTY behind it, which is exactly the condition every
        // remote transport is in.
        None
    }
}

#[cfg(test)]
#[path = "loopback_tests.rs"]
mod tests;
