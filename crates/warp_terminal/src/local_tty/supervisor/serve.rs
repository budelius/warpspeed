//! The supervisor's decision-making, separated from the machinery it drives.
//!
//! Everything platform-specific -- pseudoconsoles, descriptors, handle
//! duplication -- sits behind [`SessionHost`]. What remains here is the part
//! worth reasoning about carefully: which message produces which reply, when a
//! session's output is collected, when a dead session is dropped, and when an
//! endpoint is lent. All of that is exercised in tests against a fake host,
//! with no processes involved.

use anyhow::Result;
use warp_core::SessionId;

use super::message::{Message, SessionMetadata};
use super::session_table::SessionTable;
use super::transport::Endpoint;
use crate::event_listener::ChannelEventListener;
use crate::local_tty::screen::ScreenSnapshot;

/// How long a session survives with nobody attached.
pub const DEFAULT_SESSION_TTL_SECONDS: u64 = 24 * 60 * 60;

/// The platform machinery a supervisor drives.
///
/// Implemented over pseudoconsoles on Windows and over descriptors on Unix; a
/// fake implementation is what makes the logic above it testable.
pub trait SessionHost {
    /// Collects output a session has produced, returning what was read.
    fn read_output(&mut self, session_id: SessionId, buffer: &mut [u8]) -> Result<usize>;

    /// Whether the session's process is still alive.
    fn is_running(&mut self, session_id: SessionId) -> bool;

    /// Ends the session's process.
    fn kill(&mut self, session_id: SessionId) -> Result<()>;

    /// Lends the session's I/O endpoint to `client_pid`.
    fn lend_endpoint(&mut self, session_id: SessionId, client_pid: u32) -> Result<Endpoint>;
}

/// Holds sessions and answers clients about them.
pub struct Supervisor<H: SessionHost> {
    table: SessionTable,
    host: H,

    /// Whether the transport in use can carry an endpoint to a client.
    lends_endpoints: bool,
}

impl<H: SessionHost> Supervisor<H> {
    /// Creates a supervisor driving `host`.
    ///
    /// `lends_endpoints` comes from the transport: a client reached over a
    /// network gets streamed output rather than a borrowed descriptor.
    pub fn new(host: H, lends_endpoints: bool) -> Self {
        Self {
            table: SessionTable::new(),
            host,
            lends_endpoints,
        }
    }

    /// How many sessions are held.
    pub fn session_count(&self) -> usize {
        self.table.len()
    }

    /// Takes over a session a client is releasing.
    pub fn detach(
        &mut self,
        session_id: SessionId,
        snapshot: ScreenSnapshot,
        metadata: SessionMetadata,
        now_unix_seconds: u64,
        event_listener: ChannelEventListener,
    ) {
        self.table
            .detach(session_id, snapshot, metadata, now_unix_seconds, event_listener);
    }

    /// Answers one client message.
    ///
    /// `client_pid` is who to lend an endpoint to; it is ignored for messages
    /// that do not attach.
    pub fn handle_message(
        &mut self,
        message: &Message,
        client_pid: u32,
        now_unix_seconds: u64,
        event_listener: &ChannelEventListener,
    ) -> Option<Message> {
        match message {
            Message::Detach {
                session_id,
                snapshot,
                metadata,
            } => {
                self.table.detach(
                    *session_id,
                    snapshot.clone(),
                    metadata.clone(),
                    now_unix_seconds,
                    event_listener.clone(),
                );
                Some(Message::DetachResponse { error: None })
            }

            Message::Attach { session_id, .. } => {
                let mut result = self.table.attach(*session_id);

                // Lend only after the table has agreed to the attach, so a
                // refused attach does not hand a descriptor to a client that is
                // not going to use it.
                if let Ok(attached) = result.as_mut()
                    && self.lends_endpoints
                {
                    match self.host.lend_endpoint(*session_id, client_pid) {
                        Ok(endpoint) => attached.endpoint = Some(endpoint),
                        Err(error) => {
                            // Without an endpoint the client has nothing to read
                            // from, so this is a failed attach rather than a
                            // degraded one.
                            result = Err(format!("Failed to lend the session's endpoint: {error}"));
                        }
                    }
                }

                Some(Message::AttachResponse { result })
            }

            _ => self.table.handle_request(message, now_unix_seconds),
        }
    }

    /// Collects output from every held session and drops any whose process has
    /// ended.
    ///
    /// Returns the sessions that were dropped.
    pub fn pump(&mut self, now_unix_seconds: u64, ttl_seconds: u64) -> Vec<SessionId> {
        let mut buffer = [0u8; 64 * 1024];
        let mut ended = Vec::new();

        for session_id in self.table.session_ids() {
            match self.host.read_output(session_id, &mut buffer) {
                Ok(0) => {}
                Ok(read) => self.table.push_output(session_id, &buffer[..read]),
                Err(_) => {
                    // A read that fails means the session is unusable, which is
                    // the same outcome as its process having ended.
                    ended.push(session_id);
                    continue;
                }
            }

            if !self.host.is_running(session_id) {
                ended.push(session_id);
            }
        }

        for session_id in &ended {
            self.table.remove(*session_id);
        }

        // Reaping is folded in here so a caller cannot pump output forever and
        // never expire anything.
        let mut expired = self.table.reap_older_than(now_unix_seconds, ttl_seconds);
        for session_id in &expired {
            let _ = self.host.kill(*session_id);
        }

        ended.append(&mut expired);
        ended
    }
}

#[cfg(test)]
#[path = "serve_tests.rs"]
mod tests;
