//! One description of a window's terminal panes, used by everything that has
//! to reason about which sessions matter.
//!
//! Three callers ask nearly the same question and used to answer it three
//! ways: the exit path deciding what outlives the app, a window close deciding
//! what to hand over rather than kill, and `sessions panes` reporting what
//! is there. Where those answers disagreed, the disagreement was invisible --
//! the CLI would say a session survives a quit and the quit would kill it.
//!
//! [`TerminalPaneInfo::survives_quit`] is the shared verdict, computed once
//! during the traversal, so asking about it ahead of time and acting on it at
//! exit cannot drift apart.

use warpui::EntityId;

use crate::pane_group::PaneId;

/// A single terminal pane of one window.
#[derive(Debug, Clone)]
pub(crate) struct TerminalPaneInfo {
    /// The pane group -- one per tab -- that holds this pane.
    pub(crate) pane_group_id: EntityId,

    /// Which pane within that group.
    pub(crate) pane_id: PaneId,

    /// The tab's position in the window, counting from zero.
    pub(crate) tab_index: usize,

    /// The supervisor session behind this pane's shell.
    ///
    /// `None` means the shell is a child of this Warp process: the supervisor
    /// was unreachable when the pane started. Such a pane cannot be detached
    /// and cannot outlive the app, however busy it is.
    pub(crate) session_id: Option<warp_core::SessionId>,

    /// Whether a tracked CLI agent is running in the pane.
    pub(crate) has_running_agent: bool,

    /// Whether a foreground command is still running in the pane.
    pub(crate) is_busy: bool,

    /// Whether this session is one that must outlive the window.
    ///
    /// True for a pane with work in it the user would lose -- an agent
    /// mid-turn, a build still going -- and for every pane of a launch-config
    /// session window, whose whole point is that reopening it finds the same
    /// shells. A pane sitting at a prompt is not kept: closing a window has
    /// always been allowed to end an idle shell.
    pub(crate) survives_quit: bool,

    /// The agent conversation's title, when there is one.
    pub(crate) title: String,

    /// The shell's name, already reduced from its full path.
    pub(crate) shell: String,

    /// Where the pane last was.
    pub(crate) working_directory: String,

    /// The pane's terminal view.
    pub(crate) terminal_view_id: EntityId,

    /// The running agent, by its first command prefix, or the restored agent still to resume.
    pub(crate) agent: Option<String>,

    /// The agent's session id, as its hooks reported it or as recovered from its history, or the
    /// restored agent's saved one.
    pub(crate) agent_session_id: Option<String>,

    /// The directory the agent's hooks reported.
    pub(crate) agent_cwd: Option<String>,

    /// The session file the agent last reported.
    pub(crate) transcript_path: Option<String>,

    /// Whether the pane was restored with an agent it has not resumed yet, its tab not having
    /// been opened: there is no live agent session, only the one saved to resume.
    pub(crate) agent_not_resumed: bool,
}
