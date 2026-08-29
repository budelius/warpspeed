//! The supervisor wire protocol, version 2.
//!
//! Data only: nothing here performs I/O, so the same messages travel over a
//! Unix socket, a Windows named pipe, or a network tunnel.

use serde::{Deserialize, Serialize};
use warp_core::SessionId;

use crate::local_tty::screen::ScreenSnapshot;

/// The protocol version, carried in the socket and pipe name so a client never
/// speaks v2 to a v1 supervisor.
pub const PROTOCOL_VERSION: u32 = 2;

/// A message in either direction between a client and the supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    /// The client is releasing a session. The screen travels with it, because
    /// the supervisor is blind while a client is attached and has no other way
    /// to know what the session was displaying.
    Detach {
        session_id: SessionId,
        snapshot: ScreenSnapshot,
    },

    /// The result of a [`Message::Detach`].
    DetachResponse { error: Option<String> },

    /// The client wants a session back, resuming output after `after_sequence`.
    /// `None` asks for whatever the supervisor still holds.
    Attach {
        session_id: SessionId,
        after_sequence: Option<u64>,
    },

    /// The result of a [`Message::Attach`].
    AttachResponse {
        result: Result<AttachedSession, String>,
    },

    /// Asks for every session the supervisor is holding.
    ListSessions,

    /// The result of a [`Message::ListSessions`].
    ListSessionsResponse { sessions: Vec<SessionSummary> },

    /// Liveness check, also used to learn a supervisor's protocol version.
    Ping,

    /// The reply to [`Message::Ping`].
    Pong { protocol_version: u32 },
}

/// What a client receives when it takes a session back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedSession {
    /// The session's screen as of `sequence`.
    pub snapshot: ScreenSnapshot,

    /// Output produced after that screen was rendered.
    pub buffered_output: Vec<u8>,

    /// The sequence number `buffered_output` ends at.
    pub sequence: u64,

    /// `true` when output was discarded because the ring wrapped, so the pane
    /// can say so rather than presenting a silent gap as continuous output.
    pub output_was_dropped: bool,
}

/// A session as shown in the detached-sessions drawer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Identifies the session for a later attach.
    pub session_id: SessionId,

    /// The shell the session is running.
    pub shell: String,

    /// Where the session was last known to be.
    pub working_directory: String,

    /// The session's title, for telling two shells apart at a glance.
    pub title: String,

    /// How long the session has been detached.
    pub detached_for_seconds: u64,
}
