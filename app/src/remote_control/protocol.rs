//! What a `sessions` process and a running Warp window say to each other.
//!
//! Both ends are the same executable, so this is not a compatibility surface in
//! the usual sense -- but the two processes are frequently *different builds*
//! of it, because the whole point is to drive an installed Warp from a shell.
//! A mismatched pair must fail by not finding each other, not by
//! misinterpreting bytes, which is why [`PROTOCOL_VERSION`] is in the channel
//! name rather than in a handshake.

use serde::{Deserialize, Serialize};

/// Bumped whenever a variant's meaning or layout changes.
///
/// Adding a variant at the end of an enum is safe for bincode only in one
/// direction, and the two ends here can be either older or newer than one
/// another, so treat any change as a bump.
pub const PROTOCOL_VERSION: u32 = 2;

/// A request from the `sessions` command to a running window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// Confirms the instance is answering, and says what it is.
    Ping,

    /// Every terminal pane this instance has open.
    ListPanes,

    /// Hands one pane's session to the supervisor and closes the pane.
    DetachSession { session_id: u64 },

    /// Opens a tab attached to a session the supervisor is holding.
    AttachSession { session_id: u64 },

    /// The saved sessions this instance can reopen, newest first.
    ListStoredSessions,

    /// Reopens the newest autosave of the saved session called `name`, exactly as picking it in
    /// the session picker would: running shells are taken over and agents whose process has
    /// ended are resumed.
    OpenStoredSession { name: String },

    /// Quits, running the same teardown a user quitting the app would.
    Quit {
        /// Close every window rather than asking the app to terminate.
        ///
        /// These are genuinely different paths and the difference is the whole
        /// reason this is a parameter. On Windows and Linux a user quits by
        /// closing windows, and the event loop ends only once the last one has
        /// gone -- so `true` is what a real quit does there. Asking the app to
        /// terminate skips the per-window teardown entirely and is what Cmd-Q
        /// does on macOS.
        ///
        /// A test of the exit path that only ever took one of them would be
        /// silent about the other, which is exactly how the window-close path
        /// came to be broken without anyone noticing.
        close_windows: bool,
    },

    /// Moves the agent session in a pane to AgentCtrl without asking for confirmation, as
    /// confirming the move in that pane would.
    MoveSessionToAgentCtrl {
        terminal_view_id: u64,
        /// The agent's directory as the caller recovered it, used only when the pane's shell
        /// reports none: a restored pane, or one whose directory has gone, still holds the agent.
        cwd: Option<String>,
    },

    /// How far the move of a pane's session to AgentCtrl has got.
    MoveStatus { terminal_view_id: u64 },
}

/// A running window's answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    /// The answer to [`Request::Ping`].
    Pong {
        pid: u32,
        /// The build's version string, so a caller can tell an installed Warp
        /// from one running out of a build directory.
        version: String,
    },

    /// The answer to [`Request::ListPanes`].
    Panes { panes: Vec<PaneSummary> },

    /// The answer to [`Request::ListStoredSessions`].
    StoredSessions { sessions: Vec<StoredSessionInfo> },

    /// The request was carried out.
    ///
    /// Deliberately says nothing more. A detach that reports success means the
    /// supervisor confirmed it owns the session; there is no partial outcome to
    /// describe.
    Done,

    /// The request was understood and could not be carried out.
    Failed { error: String },

    /// The answer to [`Request::MoveSessionToAgentCtrl`]: the move began, and
    /// [`Request::MoveStatus`] follows it from here.
    MoveStarted,

    /// The answer to [`Request::MoveStatus`].
    MoveState {
        /// One of `preparing`, `importing`, `waiting_for_resume`, `cutting_over`, `done:<job>`,
        /// `failed:<reason>`, or `none` when the pane has no move in flight or finished.
        step: String,
        /// The handoff branch as `<remote>/<branch>`, once it has been pushed.
        pushed_branch: Option<String>,
    },
}

/// One terminal pane of a running instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneSummary {
    /// The session id, when the shell belongs to the supervisor.
    ///
    /// `None` means this pane's shell is a child of the Warp process itself --
    /// the supervisor was unreachable when it started, and it cannot be
    /// detached or survive a quit. Reporting that as a real state rather than
    /// omitting the pane is the point: it is the first thing to check when a
    /// session did not come back.
    pub session_id: Option<u64>,

    /// Which window holds it, as an opaque label for telling two windows of
    /// one instance apart.
    pub window: String,

    /// Which tab of that window, counting from zero.
    pub tab_index: usize,

    /// The pane's title, usually the running command or agent.
    pub title: String,

    /// The shell it is running.
    pub shell: String,

    /// Where it last was.
    pub working_directory: String,

    /// Whether a tracked CLI agent is running in it.
    pub has_running_agent: bool,

    /// Whether a foreground command is still running in it.
    pub is_busy: bool,

    /// Whether quitting Warp right now would keep this session alive.
    ///
    /// Answered by the same predicate the exit path itself uses, so this is not
    /// a second opinion about the rule -- it is the rule, asked ahead of time.
    pub survives_quit: bool,

    /// The pane's terminal view, which move requests name it by.
    pub terminal_view_id: Option<u64>,

    /// The running agent, by the command that starts it: `claude`, `pi`, `codex`, ...
    pub agent: Option<String>,

    /// The agent's own session id, as its hooks reported it or as recovered from its history.
    pub agent_session_id: Option<String>,

    /// The directory the agent's hooks reported, which can differ from the shell's.
    pub agent_cwd: Option<String>,

    /// The session file the agent last reported.
    pub transcript_path: Option<String>,
}

/// One saved session that can be reopened.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSessionInfo {
    pub name: String,
    /// Whether this is the session's running autosave rather than a stored snapshot of it.
    pub autosaved: bool,
    pub saved_at_unix_seconds: i64,
    pub tabs: usize,
    pub panes: usize,
    pub agents: usize,
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
