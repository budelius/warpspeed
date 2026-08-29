//! The seam between the supervisor protocol and how its bytes travel.
//!
//! A local transport can hand the client the PTY endpoint itself -- a file
//! descriptor on Unix, a duplicated handle on Windows -- so the app performs
//! its own I/O and the supervisor stays out of the hot path. That is
//! deliberate; see the rationale at `local_tty/server/mod.rs:124`.
//!
//! No transport that leaves the machine can pass a descriptor. So lending is a
//! capability rather than a requirement: [`Transport::lend_endpoint`] returns
//! `None` when the transport has no such power, and the caller streams output
//! through [`Message`]s instead. Everything above this trait is identical
//! either way, which is what makes a remote transport possible at all.

use anyhow::Result;
use warp_core::SessionId;

use super::message::Message;

/// A borrowed PTY endpoint.
///
/// The contents are platform-specific; callers hand this back to the platform
/// layer rather than interpreting `raw` themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoint {
    /// The child process id, which the app needs on every platform.
    pub pid: u32,

    /// A file descriptor on Unix, a handle value on Windows.
    pub raw: i64,
}

/// A bidirectional channel carrying [`Message`]s to and from the supervisor.
pub trait Transport {
    /// Sends one message.
    fn send(&mut self, message: &Message) -> Result<()>;

    /// Receives one message, or `None` if a whole one has not arrived yet.
    fn receive(&mut self) -> Result<Option<Message>>;

    /// Borrows the session's PTY endpoint, when this transport can do that.
    ///
    /// `None` means the capability does not exist here -- which is different
    /// from an attempt that failed, reported as `Some(Err(..))`. Callers that
    /// see `None` must stream instead of waiting for a descriptor that is never
    /// coming.
    fn lend_endpoint(&mut self, session_id: SessionId) -> Option<Result<Endpoint>>;
}
