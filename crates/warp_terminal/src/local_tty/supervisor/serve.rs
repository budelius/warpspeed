//! The supervisor's decision-making, separated from the machinery it drives.
//!
//! Everything platform-specific -- pseudoconsoles, descriptors, handle
//! duplication -- sits behind [`SessionHost`]. What remains here is the part
//! worth reasoning about carefully: which message produces which reply, when a
//! session's output is collected, when a dead session is dropped, and when an
//! endpoint is lent. All of that is exercised in tests against a fake host,
//! with no processes involved.

use anyhow::Result;
use log::{info, warn};
use warp_core::SessionId;

use super::message::{Message, SessionMetadata};
use super::session_table::SessionTable;
use super::transport::Endpoint;
use crate::event_listener::ChannelEventListener;
use crate::local_tty::PtyOptions;
use crate::local_tty::screen::ScreenSnapshot;

/// How long a session survives with nobody attached.
pub const DEFAULT_SESSION_TTL_SECONDS: u64 = 24 * 60 * 60;

/// The platform machinery a supervisor drives.
///
/// Implemented over pseudoconsoles on Windows and over descriptors on Unix; a
/// fake implementation is what makes the logic above it testable.
pub trait SessionHost {
    /// Starts a terminal owned by this host and lends its endpoint to `client_pid`.
    fn spawn(
        &mut self,
        session_id: SessionId,
        options: PtyOptions,
        client_pid: u32,
    ) -> Result<Endpoint>;

    /// Marks the app's borrowed endpoint as released.
    fn detach(&mut self, session_id: SessionId) -> Result<()>;

    /// Returns sessions whose attached client process disappeared without detaching.
    fn take_orphaned_sessions(&mut self) -> Vec<SessionId>;

    /// Collects output a session has produced, returning what was read.
    fn read_output(&mut self, session_id: SessionId, buffer: &mut [u8]) -> Result<usize>;

    /// Whether the session's process is still alive.
    fn is_running(&mut self, session_id: SessionId) -> bool;

    /// Changes the session's terminal size.
    fn resize(&mut self, session_id: SessionId, columns: u16, rows: u16) -> Result<()>;

    /// Ends the session's process.
    fn kill(&mut self, session_id: SessionId) -> Result<()>;

    /// Lends the session's I/O endpoint to `client_pid`.
    fn lend_endpoint(&mut self, session_id: SessionId, client_pid: u32) -> Result<Endpoint>;

    /// Whether the host still owns any terminal processes.
    fn has_sessions(&self) -> bool;
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

    /// How many detached sessions are held.
    pub fn session_count(&self) -> usize {
        self.table.len()
    }

    /// Whether the host still owns any terminal process, attached or detached.
    pub fn has_host_sessions(&self) -> bool {
        self.host.has_sessions()
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
        self.table.detach(
            session_id,
            snapshot,
            metadata,
            now_unix_seconds,
            event_listener,
        );
    }

    /// Takes over sessions whose client process exited without sending `Detach`.
    pub fn recover_orphaned_sessions(
        &mut self,
        now_unix_seconds: u64,
        event_listener: &ChannelEventListener,
    ) {
        for session_id in self.host.take_orphaned_sessions() {
            info!("Terminal session {session_id:?} lost its client without a detach; holding it");
            self.table.detach(
                session_id,
                ScreenSnapshot {
                    columns: 80,
                    rows: 24,
                    repaint: Vec::new(),
                },
                // Nothing is known about a session whose client vanished
                // without handing it over: the announcement it would have
                // carried lives in the client that died with it.
                SessionMetadata::default(),
                now_unix_seconds,
                event_listener.clone(),
            );
        }
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
            Message::Spawn {
                session_id,
                options,
            } => {
                let result = bincode::deserialize::<PtyOptions>(options)
                    .map_err(anyhow::Error::from)
                    .and_then(|options| self.host.spawn(*session_id, options, client_pid))
                    .map_err(|error| error.to_string());
                match &result {
                    Ok(endpoint) => info!(
                        "Spawned terminal session {session_id:?} (shell pid {}) for client {client_pid}",
                        endpoint.pid
                    ),
                    Err(error) => warn!(
                        "Failed to spawn terminal session {session_id:?} for client {client_pid}: {error}"
                    ),
                }
                Some(Message::SpawnResponse { result })
            }

            Message::Detach {
                session_id,
                snapshot,
                metadata,
            } => {
                let error = self
                    .host
                    .detach(*session_id)
                    .err()
                    .map(|error| error.to_string());
                info!(
                    "Client {client_pid} detached terminal session {session_id:?}: {}",
                    error.as_deref().unwrap_or("ok")
                );
                if error.is_none() {
                    self.table.detach(
                        *session_id,
                        snapshot.clone(),
                        metadata.clone(),
                        now_unix_seconds,
                        event_listener.clone(),
                    );
                }
                Some(Message::DetachResponse { error })
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
                            self.table.mark_detached(*session_id);
                            result = Err(format!("Failed to lend the session's endpoint: {error}"));
                        }
                    }
                }

                match &result {
                    Ok(_) => info!("Client {client_pid} attached terminal session {session_id:?}"),
                    Err(error) => warn!(
                        "Client {client_pid} failed to attach terminal session {session_id:?}: {error}"
                    ),
                }
                Some(Message::AttachResponse { result })
            }

            Message::Resize {
                session_id,
                columns,
                rows,
            } => {
                let error = self
                    .host
                    .resize(*session_id, *columns, *rows)
                    .err()
                    .map(|error| error.to_string());
                if error.is_none() {
                    // While detached the supervisor owns both ConPTY and its
                    // headless screen. Keep them at the receiving pane's size
                    // so attach can always replay a bounds-safe current screen.
                    self.table.resize_detached(*session_id, *columns, *rows);
                }
                Some(Message::ResizeResponse { error })
            }

            Message::Kill { session_id } => {
                // Whether the shell was alive when the kill arrived separates a client cleaning up
                // after an exit from a client ending a live shell.
                let was_running = self.host.is_running(*session_id);
                self.table.remove(*session_id);
                let error = self
                    .host
                    .kill(*session_id)
                    .err()
                    .map(|error| error.to_string());
                info!(
                    "Client {client_pid} killed terminal session {session_id:?} (shell was running: {was_running}): {}",
                    error.as_deref().unwrap_or("ok")
                );
                Some(Message::KillResponse { error })
            }

            Message::ChildStatus { session_id } => Some(Message::ChildStatusResponse {
                running: self.host.is_running(*session_id),
            }),

            Message::SpawnResponse { .. }
            | Message::DetachResponse { .. }
            | Message::AttachResponse { .. }
            | Message::ResizeResponse { .. }
            | Message::KillResponse { .. }
            | Message::ChildStatusResponse { .. }
            | Message::ListSessionsResponse { .. }
            | Message::Pong { .. }
            | Message::ResponseReceived => None,

            Message::ListSessions | Message::Ping => {
                self.table.handle_request(message, now_unix_seconds)
            }
        }
    }

    /// Collects output from every held session and drops any whose process has
    /// ended.
    ///
    /// Returns the sessions that were dropped.
    pub fn pump(&mut self, now_unix_seconds: u64, ttl_seconds: u64) -> Vec<SessionId> {
        let mut buffer = [0u8; 64 * 1024];
        let mut ended = Vec::new();

        for session_id in self.table.detached_session_ids() {
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
            info!("Detached terminal session {session_id:?} ended");
            self.table.remove(*session_id);
        }

        // Reaping is folded in here so a caller cannot pump output forever and
        // never expire anything.
        let mut expired = self.table.reap_older_than(now_unix_seconds, ttl_seconds);
        for session_id in &expired {
            info!(
                "Detached terminal session {session_id:?} expired after {ttl_seconds}s; killing it"
            );
            let _ = self.host.kill(*session_id);
        }

        ended.append(&mut expired);
        ended
    }
}

#[cfg(test)]
#[path = "serve_tests.rs"]
mod tests;
