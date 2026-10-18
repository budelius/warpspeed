//! The cross-platform half of the terminal session supervisor.
//!
//! `local_tty::server` is Unix-only because it passes file descriptors over a
//! Unix domain socket. Message shapes, framing, and the transport seam are not
//! Unix-specific, so they live here where the Windows backend can share them.

pub mod compatibility;
pub mod framing;
pub mod loopback;
pub mod message;
pub mod ring;
pub mod serve;
pub mod session_table;
pub mod transport;

pub use message::{AttachedSession, Message, PROTOCOL_VERSION, SessionMetadata, SessionSummary};
