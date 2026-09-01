//! What the supervisor holds for each detached session, and how it answers
//! [`Message`]s about them.
//!
//! Deliberately free of processes, sockets, and clocks: a session here is a
//! screen, a ring of output, and some metadata. That keeps the behaviour that
//! decides what a reattached pane sees testable without spawning anything, and
//! lets the Unix and Windows backends share all of it.
//!
//! ## What a reattaching client receives
//!
//! The contract mirrors the property proved for [`HeadlessScreen`]: applying
//! `snapshot` and then feeding `buffered_output` reproduces exactly what a
//! client that never detached would have seen.
//!
//! - `snapshot` is the screen as of `snapshot_sequence`.
//! - `buffered_output` is every byte from `snapshot_sequence` to `sequence`.
//!
//! When the ring has discarded bytes the detach-point screen would need, that
//! reproduction is impossible. Rather than hand back a stream with a hole in
//! it, the supervisor sends the *current* screen with no buffered output and
//! sets `output_was_dropped`, so the pane can say that intervening scrollback
//! was lost instead of presenting a gap as continuous output.

use std::collections::HashMap;

use warp_core::SessionId;

use super::message::{AttachedSession, Message, SessionMetadata, SessionSummary};
use super::ring::OutputRing;
use crate::event_listener::ChannelEventListener;
use crate::local_tty::screen::{HeadlessScreen, ScreenSnapshot};

/// How much output to retain per detached session.
pub const DEFAULT_RING_CAPACITY_BYTES: usize = 4 * 1024 * 1024;

/// A session the supervisor is holding on a client's behalf.
pub struct SupervisedSession {
    /// The screen, kept current as output arrives, so it can be handed back
    /// even when the ring has wrapped.
    screen: HeadlessScreen,

    /// Raw output since the session was detached.
    ring: OutputRing,

    /// The screen as of the moment the client let go.
    snapshot: ScreenSnapshot,

    /// The ring position `snapshot` corresponds to.
    snapshot_sequence: u64,

    /// Describes the session in the drawer.
    metadata: SessionMetadata,

    /// When the session was detached, in Unix seconds. Supplied by the caller
    /// rather than read from a clock, so tests stay deterministic.
    detached_at_unix_seconds: u64,

    /// `true` while a client holds this session.
    attached: bool,
}

impl SupervisedSession {
    /// The session's current screen text, for logging and for describing what
    /// is running in the drawer.
    pub fn visible_text(&self) -> String {
        self.screen.visible_text()
    }
}

/// Every session the supervisor is holding.
#[derive(Default)]
pub struct SessionTable {
    sessions: HashMap<SessionId, SupervisedSession>,
}

impl SessionTable {
    /// Creates an empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many sessions are held.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether any session is held.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Takes over a session that a client is releasing.
    ///
    /// The snapshot is the client's screen at the moment it let go; from here
    /// on the supervisor keeps that screen current itself.
    #[allow(clippy::too_many_arguments)]
    pub fn detach(
        &mut self,
        session_id: SessionId,
        snapshot: ScreenSnapshot,
        metadata: SessionMetadata,
        detached_at_unix_seconds: u64,
        event_listener: ChannelEventListener,
    ) {
        let screen = HeadlessScreen::from_snapshot(&snapshot, event_listener);

        self.sessions.insert(
            session_id,
            SupervisedSession {
                screen,
                ring: OutputRing::new(DEFAULT_RING_CAPACITY_BYTES),
                snapshot,
                snapshot_sequence: 0,
                metadata,
                detached_at_unix_seconds,
                attached: false,
            },
        );
    }

    /// Records output produced by a detached session.
    ///
    /// The bytes go to both the ring, so they can be replayed, and the screen,
    /// so a client can be given something correct even after the ring wraps.
    pub fn push_output(&mut self, session_id: SessionId, bytes: &[u8]) {
        let Some(session) = self.sessions.get_mut(&session_id) else {
            return;
        };
        session.ring.push(bytes);
        session.screen.feed(bytes);
    }

    /// Hands a session back to a client.
    pub fn attach(&mut self, session_id: SessionId) -> Result<AttachedSession, String> {
        let Some(session) = self.sessions.get_mut(&session_id) else {
            return Err("Terminal session was not found".to_owned());
        };
        if session.attached {
            return Err("Terminal session is already attached".to_owned());
        }

        let (buffered_output, dropped) = session.ring.read_after(Some(session.snapshot_sequence));
        session.attached = true;

        if dropped {
            // The detach-point screen can no longer be caught up to the
            // present, so send the present instead and say so.
            Ok(AttachedSession {
                snapshot: session.screen.snapshot(),
                buffered_output: Vec::new(),
                sequence: session.ring.sequence(),
                output_was_dropped: true,
                endpoint: None,
            })
        } else {
            Ok(AttachedSession {
                snapshot: session.snapshot.clone(),
                buffered_output,
                sequence: session.ring.sequence(),
                output_was_dropped: false,
                endpoint: None,
            })
        }
    }

    /// Every held session, for the detached-sessions drawer.
    pub fn list(&self, now_unix_seconds: u64) -> Vec<SessionSummary> {
        let mut summaries: Vec<SessionSummary> = self
            .sessions
            .iter()
            .map(|(session_id, session)| SessionSummary {
                session_id: *session_id,
                metadata: session.metadata.clone(),
                detached_for_seconds: now_unix_seconds
                    .saturating_sub(session.detached_at_unix_seconds),
            })
            .collect();

        // A stable order, so the drawer does not reshuffle between refreshes.
        summaries.sort_by_key(|summary| summary.session_id.as_u64());
        summaries
    }

    /// Every detached session's id, in a stable order.
    pub fn detached_session_ids(&self) -> Vec<SessionId> {
        let mut ids: Vec<SessionId> = self
            .sessions
            .iter()
            .filter_map(|(id, session)| (!session.attached).then_some(*id))
            .collect();
        ids.sort_by_key(|id| id.as_u64());
        ids
    }

    /// Makes a failed attach retryable.
    pub fn mark_detached(&mut self, session_id: SessionId) {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.attached = false;
        }
    }

    /// Stops holding a session.
    pub fn remove(&mut self, session_id: SessionId) -> bool {
        self.sessions.remove(&session_id).is_some()
    }

    /// Discards sessions detached longer ago than `max_age_seconds`.
    ///
    /// Returns the sessions dropped, so the caller can kill their processes.
    pub fn reap_older_than(
        &mut self,
        now_unix_seconds: u64,
        max_age_seconds: u64,
    ) -> Vec<SessionId> {
        let expired: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, session)| {
                !session.attached
                    && now_unix_seconds.saturating_sub(session.detached_at_unix_seconds)
                        > max_age_seconds
            })
            .map(|(id, _)| *id)
            .collect();

        for id in &expired {
            self.sessions.remove(id);
        }
        expired
    }

    /// Answers a request, or returns `None` for a message that is not one.
    pub fn handle_request(&mut self, request: &Message, now_unix_seconds: u64) -> Option<Message> {
        match request {
            Message::Attach { session_id, .. } => Some(Message::AttachResponse {
                result: self.attach(*session_id),
            }),
            Message::ListSessions => Some(Message::ListSessionsResponse {
                sessions: self.list(now_unix_seconds),
            }),
            Message::Ping => Some(Message::Pong {
                protocol_version: super::message::PROTOCOL_VERSION,
            }),
            // Detach and process lifecycle requests need the platform host owned by `Supervisor`.
            Message::Spawn { .. }
            | Message::SpawnResponse { .. }
            | Message::Detach { .. }
            | Message::DetachResponse { .. }
            | Message::AttachResponse { .. }
            | Message::Resize { .. }
            | Message::ResizeResponse { .. }
            | Message::Kill { .. }
            | Message::KillResponse { .. }
            | Message::ChildStatus { .. }
            | Message::ChildStatusResponse { .. }
            | Message::ListSessionsResponse { .. }
            | Message::Pong { .. }
            | Message::ResponseReceived => None,
        }
    }
}

#[cfg(test)]
#[path = "session_table_tests.rs"]
mod tests;
