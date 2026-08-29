//! The cross-platform half of the terminal session supervisor.
//!
//! `local_tty::server` is Unix-only because it passes file descriptors over a
//! Unix domain socket. Message shapes, framing, and the transport seam are not
//! Unix-specific, so they live here where the Windows backend can share them.

pub mod framing;
pub mod message;

pub use message::{AttachedSession, Message, PROTOCOL_VERSION, SessionSummary};
