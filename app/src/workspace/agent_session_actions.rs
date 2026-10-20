//! What clicking an agent-session row does.

use std::path::{Path, PathBuf};

use warp_util::path::ShellFamily;

use crate::app_state::{CliAgentResumeDescriptor, SessionHolder};
use crate::terminal::agent_session_handoff::{HandoffSource, agent_ctrl_profile};
use crate::terminal::agent_session_history::is_safe_session_id;
use crate::terminal::agent_sessions_index::{AgentSessionRow, AgentSessionState};
use crate::terminal::cli_agent::CLIAgent;
use crate::terminal::shell::ShellType;
use crate::terminal::terminal_manager::HeldAgent;
use crate::workspace::PaneViewLocator;

/// What a row's click does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentSessionAction {
    Focus(PaneViewLocator),
    Start(PaneViewLocator),
    Attach {
        session_id: warp_core::SessionId,
        holder: Option<SessionHolder>,
        agent: Option<HeldAgent>,
    },
    Managed {
        job_id: String,
        target: Option<PaneViewLocator>,
    },
    /// `target` is the pane to resume in, or to split beside when it is busy.
    Resume {
        agent: CLIAgent,
        session_id: String,
        cwd: PathBuf,
        target: Option<PaneViewLocator>,
    },
}

impl AgentSessionAction {
    /// The verb a row shows for this action.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            AgentSessionAction::Focus(_) => "Focus",
            AgentSessionAction::Start(_) => "Start",
            AgentSessionAction::Attach { .. } => "Attach",
            AgentSessionAction::Managed { .. } => "Open",
            AgentSessionAction::Resume { .. } => "Resume",
        }
    }
}

/// The action for `row`, or `None` when nothing can reopen it.
pub(crate) fn action_for(
    row: &AgentSessionRow,
    target: Option<PaneViewLocator>,
) -> Option<AgentSessionAction> {
    match &row.state {
        AgentSessionState::Running { locator } => Some(AgentSessionAction::Focus(*locator)),
        AgentSessionState::Pending { locator } => Some(AgentSessionAction::Start(*locator)),
        AgentSessionState::Detached {
            session_id,
            instance_pid: None,
            holder,
        } => Some(AgentSessionAction::Attach {
            session_id: *session_id,
            holder: holder.clone(),
            agent: row
                .agent
                .zip(row.session_id.clone())
                .map(|(agent, session_id)| HeldAgent { agent, session_id }),
        }),
        AgentSessionState::RunningInOtherInstance { .. }
        | AgentSessionState::Detached {
            instance_pid: Some(_),
            ..
        }
        | AgentSessionState::KnownProcess { .. }
        | AgentSessionState::HeldByOtherBuild { .. }
        | AgentSessionState::NoKnownPane { .. } => None,
        AgentSessionState::Past => {
            let agent = row.agent?;
            let session_id = row.session_id.clone().filter(|id| is_safe_session_id(id))?;
            resume_descriptor(agent, &session_id, &row.cwd).resume_command()?;
            Some(AgentSessionAction::Resume {
                agent,
                session_id,
                cwd: row.cwd.clone(),
                target,
            })
        }
    }
}

/// The recorded conversation a past row can start on AgentCtrl.
pub(crate) fn agent_ctrl_handoff_source(row: &AgentSessionRow) -> Option<HandoffSource> {
    if !matches!(row.state, AgentSessionState::Past) || row.cwd.as_os_str().is_empty() {
        return None;
    }
    let agent = row.agent?;
    agent_ctrl_profile(agent)?;
    let session_id = row.session_id.clone().filter(|id| is_safe_session_id(id))?;
    Some(HandoffSource {
        agent,
        session_id,
        cwd: row.cwd.clone(),
        transcript_path: None,
    })
}

pub(crate) fn resume_descriptor(
    agent: CLIAgent,
    session_id: &str,
    cwd: &Path,
) -> CliAgentResumeDescriptor {
    CliAgentResumeDescriptor {
        agent,
        session_id: Some(session_id.to_owned()),
        cwd: cwd.to_path_buf(),
        title: None,
        captured_unix_seconds: None,
    }
}

/// The conversation's own folder while it exists, else `None`: agents such as Claude look a
/// conversation up by the folder it was recorded in, so resuming it anywhere else fails.
pub(crate) fn resume_directory(cwd: &Path, is_dir: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    (!cwd.as_os_str().is_empty() && is_dir(cwd)).then(|| cwd.to_path_buf())
}

/// A change to `cwd` joined to `resume` so that `resume` runs only once the change succeeded.
pub(crate) fn resume_command_line(cwd: &Path, resume: &str, shell: ShellKind) -> String {
    let cwd = cwd.to_string_lossy();
    match shell {
        ShellKind::Posix => format!("cd {} && {resume}", ShellFamily::Posix.shell_escape(&cwd)),
        ShellKind::Fish => format!("cd {}; and {resume}", ShellFamily::Posix.shell_escape(&cwd)),
        // `-LiteralPath` keeps `[` and `]` from being read as wildcards, and a terminating error
        // stops the line before the agent starts in the wrong folder.
        ShellKind::PowerShell => format!(
            "Set-Location -LiteralPath '{}' -ErrorAction Stop; {resume}",
            cwd.replace('\'', "''")
        ),
    }
}

/// The shells as far as joining two commands goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShellKind {
    Posix,
    Fish,
    PowerShell,
}

impl From<ShellType> for ShellKind {
    fn from(shell: ShellType) -> Self {
        match shell {
            ShellType::Zsh | ShellType::Bash => ShellKind::Posix,
            ShellType::Fish => ShellKind::Fish,
            ShellType::PowerShell => ShellKind::PowerShell,
        }
    }
}

impl From<ShellFamily> for ShellKind {
    /// Fish is indistinguishable from the other POSIX shells here; fish 3 accepts `&&` too.
    fn from(family: ShellFamily) -> Self {
        match family {
            ShellFamily::Posix => ShellKind::Posix,
            ShellFamily::PowerShell => ShellKind::PowerShell,
        }
    }
}

#[cfg(test)]
#[path = "agent_session_actions_tests.rs"]
mod tests;
