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
use serde::{Deserialize, Serialize};

use super::message::Message;

/// A borrowed PTY endpoint.
///
/// The contents are platform-specific; callers hand this back to the platform
/// layer rather than interpreting `raw` themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    /// The child process id, which the app needs on every platform.
    pub pid: u32,

    /// A file descriptor on Unix, a handle value on Windows. Meaningful only
    /// inside the process the endpoint was lent to.
    pub raw: i64,
}

/// A bidirectional channel carrying [`Message`]s to and from the supervisor.
///
/// ## Why lending is not a method here
///
/// Lending is something the *supervisor* does to a client, not something a
/// client pulls. On Windows the supervisor duplicates its handle into the
/// client process and the resulting value travels inside the attach response;
/// on Unix the descriptor travels beside that response over `SCM_RIGHTS`, and
/// the transport fills it in as it decodes. Either way the endpoint reaches the
/// caller through [`super::message::AttachedSession`], so the trait only has to
/// say whether this transport can carry one at all.
pub trait Transport {
    /// Sends one message.
    fn send(&mut self, message: &Message) -> Result<()>;

    /// Receives one message, or `None` if a whole one has not arrived yet.
    fn receive(&mut self) -> Result<Option<Message>>;

    /// Whether an endpoint can reach a client over this transport.
    ///
    /// `false` for anything that leaves the machine: a descriptor cannot cross
    /// a network, so those callers stream output through [`Message`]s instead
    /// of waiting for an endpoint that is never coming.
    fn lends_endpoints(&self) -> bool;
}
